use std::ops::Bound;
use std::ops::Range;
use std::ops::RangeBounds;
use zellij_utils::data::StyledText;

/// Char-index ranges of every non-overlapping occurrence of `substr` in `text`.
///
/// `color_range` and friends index by char (`Text::len` is `chars().count()`), so a byte
/// offset from `str::find` cannot be handed to them directly. An empty `substr` matches
/// nowhere - `match_indices("")` would otherwise yield a match at every position.
fn substring_char_ranges(text: &str, substr: &str) -> Vec<Range<usize>> {
    if substr.is_empty() {
        return vec![];
    }
    let substr_chars = substr.chars().count();
    text.match_indices(substr)
        .map(|(byte_pos, _)| {
            let start = text[..byte_pos].chars().count();
            start..start + substr_chars
        })
        .collect()
}

#[derive(Debug, Default, Clone)]
pub struct Text {
    text: String,
    selected: bool,
    opaque: bool,
    disabled: bool,
    indices: Vec<Vec<usize>>,
}

impl From<StyledText> for Text {
    fn from(styled_text: StyledText) -> Self {
        Text {
            text: styled_text.text,
            selected: false,
            opaque: false,
            disabled: false,
            indices: styled_text.indices,
        }
    }
}

impl Text {
    pub fn new<S>(content: S) -> Self
    where
        S: AsRef<str> + ToString,
    {
        Text {
            text: content.to_string(),
            selected: false,
            opaque: false,
            disabled: false,
            indices: vec![],
        }
    }
    pub fn selected(mut self) -> Self {
        self.selected = true;
        self
    }
    pub fn opaque(mut self) -> Self {
        self.opaque = true;
        self
    }
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
    pub fn dim_indices(mut self, mut indices: Vec<usize>) -> Self {
        const DIM_LEVEL: usize = 4;
        self.pad_indices(DIM_LEVEL);
        if let Some(i) = self.indices.get_mut(DIM_LEVEL) {
            i.append(&mut indices)
        }
        self
    }
    pub fn dim_range<R: RangeBounds<usize>>(mut self, indices: R) -> Self {
        const DIM_LEVEL: usize = 4;
        self.pad_indices(DIM_LEVEL);
        let start = match indices.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s,
        };
        let end = match indices.end_bound() {
            Bound::Unbounded => self.text.chars().count(),
            Bound::Included(s) => *s + 1,
            Bound::Excluded(s) => *s,
        };
        let indices = start..end;
        if let Some(i) = self.indices.get_mut(DIM_LEVEL) {
            i.append(&mut indices.into_iter().collect())
        }
        self
    }
    pub fn dim_substring<S: AsRef<str>>(mut self, substr: S) -> Self {
        for range in substring_char_ranges(&self.text.clone(), substr.as_ref()) {
            self = self.dim_range(range);
        }
        self
    }
    pub fn dim_all(self) -> Self {
        const DIM_LEVEL: usize = 4;
        self.color_range(DIM_LEVEL, ..)
    }
    pub fn unbold_indices(mut self, mut indices: Vec<usize>) -> Self {
        const UNBOLD_LEVEL: usize = 5;
        self.pad_indices(UNBOLD_LEVEL);
        if let Some(i) = self.indices.get_mut(UNBOLD_LEVEL) {
            i.append(&mut indices)
        }
        self
    }
    pub fn unbold_range<R: RangeBounds<usize>>(mut self, indices: R) -> Self {
        const UNBOLD_LEVEL: usize = 5;
        self.pad_indices(UNBOLD_LEVEL);
        let start = match indices.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s,
        };
        let end = match indices.end_bound() {
            Bound::Unbounded => self.text.chars().count(),
            Bound::Included(s) => *s + 1,
            Bound::Excluded(s) => *s,
        };
        let indices = start..end;
        if let Some(i) = self.indices.get_mut(UNBOLD_LEVEL) {
            i.append(&mut indices.into_iter().collect())
        }
        self
    }
    pub fn unbold_substring<S: AsRef<str>>(mut self, substr: S) -> Self {
        for range in substring_char_ranges(&self.text.clone(), substr.as_ref()) {
            self = self.unbold_range(range);
        }
        self
    }
    pub fn unbold_all(self) -> Self {
        const UNBOLD_LEVEL: usize = 5;
        self.color_range(UNBOLD_LEVEL, ..)
    }
    pub fn error_color_indices(mut self, mut indices: Vec<usize>) -> Self {
        const ERROR_COLOR_LEVEL: usize = 6;
        self.pad_indices(ERROR_COLOR_LEVEL);
        if let Some(i) = self.indices.get_mut(ERROR_COLOR_LEVEL) {
            i.append(&mut indices)
        }
        self
    }
    pub fn error_color_range<R: RangeBounds<usize>>(mut self, indices: R) -> Self {
        const ERROR_COLOR_LEVEL: usize = 6;
        self.pad_indices(ERROR_COLOR_LEVEL);
        let start = match indices.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s,
        };
        let end = match indices.end_bound() {
            Bound::Unbounded => self.text.chars().count(),
            Bound::Included(s) => *s + 1,
            Bound::Excluded(s) => *s,
        };
        let indices = start..end;
        if let Some(i) = self.indices.get_mut(ERROR_COLOR_LEVEL) {
            i.append(&mut indices.into_iter().collect())
        }
        self
    }
    pub fn error_color_substring<S: AsRef<str>>(mut self, substr: S) -> Self {
        for range in substring_char_ranges(&self.text.clone(), substr.as_ref()) {
            self = self.error_color_range(range);
        }
        self
    }
    pub fn error_color_nth_substring<S: AsRef<str>>(
        self,
        substr: S,
        occurrence_index: usize,
    ) -> Self {
        const ERROR_COLOR_LEVEL: usize = 6;
        match substring_char_ranges(&self.text, substr.as_ref())
            .into_iter()
            .nth(occurrence_index)
        {
            Some(range) => self.color_range(ERROR_COLOR_LEVEL, range),
            None => self,
        }
    }

    pub fn error_color_last_substring<S: AsRef<str>>(self, substr: S) -> Self {
        const ERROR_COLOR_LEVEL: usize = 6;
        match substring_char_ranges(&self.text, substr.as_ref()).pop() {
            Some(range) => self.color_range(ERROR_COLOR_LEVEL, range),
            None => self,
        }
    }

    pub fn error_color_all(self) -> Self {
        const ERROR_COLOR_LEVEL: usize = 6;
        self.color_range(ERROR_COLOR_LEVEL, ..)
    }
    pub fn success_color_indices(mut self, mut indices: Vec<usize>) -> Self {
        const SUCCESS_COLOR_LEVEL: usize = 7;
        self.pad_indices(SUCCESS_COLOR_LEVEL);
        if let Some(i) = self.indices.get_mut(SUCCESS_COLOR_LEVEL) {
            i.append(&mut indices)
        }
        self
    }
    pub fn success_color_range<R: RangeBounds<usize>>(mut self, indices: R) -> Self {
        const SUCCESS_COLOR_LEVEL: usize = 7;
        self.pad_indices(SUCCESS_COLOR_LEVEL);
        let start = match indices.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s,
        };
        let end = match indices.end_bound() {
            Bound::Unbounded => self.text.chars().count(),
            Bound::Included(s) => *s + 1,
            Bound::Excluded(s) => *s,
        };
        let indices = start..end;
        if let Some(i) = self.indices.get_mut(SUCCESS_COLOR_LEVEL) {
            i.append(&mut indices.into_iter().collect())
        }
        self
    }
    pub fn success_color_substring<S: AsRef<str>>(mut self, substr: S) -> Self {
        for range in substring_char_ranges(&self.text.clone(), substr.as_ref()) {
            self = self.success_color_range(range);
        }
        self
    }
    pub fn success_color_nth_substring<S: AsRef<str>>(
        self,
        substr: S,
        occurrence_index: usize,
    ) -> Self {
        const SUCCESS_COLOR_LEVEL: usize = 7;
        match substring_char_ranges(&self.text, substr.as_ref())
            .into_iter()
            .nth(occurrence_index)
        {
            Some(range) => self.color_range(SUCCESS_COLOR_LEVEL, range),
            None => self,
        }
    }

    pub fn success_color_last_substring<S: AsRef<str>>(self, substr: S) -> Self {
        const SUCCESS_COLOR_LEVEL: usize = 7;
        match substring_char_ranges(&self.text, substr.as_ref()).pop() {
            Some(range) => self.color_range(SUCCESS_COLOR_LEVEL, range),
            None => self,
        }
    }

    pub fn success_color_all(self) -> Self {
        const SUCCESS_COLOR_LEVEL: usize = 7;
        self.color_range(SUCCESS_COLOR_LEVEL, ..)
    }
    pub fn color_indices(mut self, index_level: usize, mut indices: Vec<usize>) -> Self {
        self.pad_indices(index_level);
        if let Some(i) = self.indices.get_mut(index_level) {
            i.append(&mut indices)
        }
        self
    }
    pub fn color_range<R: RangeBounds<usize>>(mut self, index_level: usize, indices: R) -> Self {
        self.pad_indices(index_level);
        let start = match indices.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(s) => *s,
            Bound::Excluded(s) => *s,
        };
        let end = match indices.end_bound() {
            Bound::Unbounded => self.text.chars().count(),
            Bound::Included(s) => *s + 1,
            Bound::Excluded(s) => *s,
        };
        let indices = start..end;
        if let Some(i) = self.indices.get_mut(index_level) {
            i.append(&mut indices.into_iter().collect())
        }
        self
    }

    pub fn color_substring<S: AsRef<str>>(mut self, index_level: usize, substr: S) -> Self {
        for range in substring_char_ranges(&self.text.clone(), substr.as_ref()) {
            self = self.color_range(index_level, range);
        }
        self
    }

    pub fn color_all(self, index_level: usize) -> Self {
        self.color_range(index_level, ..)
    }

    pub fn color_nth_substring<S: AsRef<str>>(
        self,
        index_level: usize,
        substr: S,
        occurrence_index: usize,
    ) -> Self {
        match substring_char_ranges(&self.text, substr.as_ref())
            .into_iter()
            .nth(occurrence_index)
        {
            Some(range) => self.color_range(index_level, range),
            None => self,
        }
    }

    pub fn color_last_substring<S: AsRef<str>>(self, index_level: usize, substr: S) -> Self {
        match substring_char_ranges(&self.text, substr.as_ref()).pop() {
            Some(range) => self.color_range(index_level, range),
            None => self,
        }
    }

    pub fn content(&self) -> &str {
        &self.text
    }

    /// Replace the content, keeping the styling that still lands on it.
    ///
    /// For rewriting text a renderer has already been handed - clipping it to
    /// the room available, say. Colour indices past the end of the new content
    /// are dropped rather than left dangling.
    pub fn with_content<S: AsRef<str> + ToString>(mut self, content: S) -> Self {
        self.text = content.to_string();
        let characters = self.text.chars().count();
        for level in &mut self.indices {
            level.retain(|index| *index < characters);
        }
        self
    }
    fn pad_indices(&mut self, index_level: usize) {
        if self.indices.get(index_level).is_none() {
            for _ in self.indices.len()..=index_level {
                self.indices.push(vec![]);
            }
        }
    }
    pub fn serialize(&self) -> String {
        let text = self
            .text
            .to_string()
            .as_bytes()
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut indices = String::new();
        for index_variants in &self.indices {
            indices.push_str(&format!(
                "{}$",
                index_variants
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }

        let mut prefix = "".to_owned();

        if self.selected {
            prefix.push('x');
        }

        if self.opaque {
            prefix.push('z');
        }

        if self.disabled {
            prefix.push('d');
        }

        format!("{}{}{}", prefix, indices, text)
    }
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

pub fn print_text(text: Text) {
    print!("\u{1b}Pztext;{}\u{1b}\\", text.serialize())
}

pub fn print_text_with_coordinates(
    text: Text,
    x: usize,
    y: usize,
    width: Option<usize>,
    height: Option<usize>,
) {
    let width = width.map(|w| w.to_string()).unwrap_or_default();
    let height = height.map(|h| h.to_string()).unwrap_or_default();
    print!(
        "\u{1b}Pztext;{}/{}/{}/{};{}\u{1b}\\",
        x,
        y,
        width,
        height,
        text.serialize()
    )
}

pub fn serialize_text(text: &Text) -> String {
    format!("\u{1b}Pztext;{}\u{1b}\\", text.serialize())
}

pub fn serialize_text_with_coordinates(
    text: &Text,
    x: usize,
    y: usize,
    width: Option<usize>,
    height: Option<usize>,
) -> String {
    let width = width.map(|w| w.to_string()).unwrap_or_default();
    let height = height.map(|h| h.to_string()).unwrap_or_default();
    format!(
        "\u{1b}Pztext;{}/{}/{}/{};{}\u{1b}\\",
        x,
        y,
        width,
        height,
        text.serialize()
    )
}

#[cfg(test)]
mod tests {
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

    /// The empty substring used to spin forever: `find("")` returns `Some(0)` and the cursor
    /// advanced by `substr.len()` == 0.
    #[test]
    fn empty_substring_matches_nothing() {
        let serialized = Text::new("Foo bar baz").dim_substring("").serialize();
        assert_eq!(serialized, text_body("Foo bar baz"), "no indices expected");
    }
}
