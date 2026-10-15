//! Indent Scheme source while preserving comments, literals, and author-selected line breaks.

use anyhow::{anyhow, Result};
use steel::parser::{lexer::TokenStream, tokens::TokenType};

pub fn format(source: &str) -> Result<String> {
    crate::validate_source(source)?;
    let tokens = TokenStream::new(source, false, None)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("tokenize Scheme for formatting: {error:?}"))?;
    let mut output = String::new();
    let mut depth = 0usize;
    let mut token_index: usize = 0;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        let inside_token = token_index
            .checked_sub(1)
            .and_then(|index| tokens.get(index))
            .is_some_and(|token| token.span().end as usize > offset);
        if inside_token {
            output.push_str(line);
        } else {
            let content = line.trim_start_matches([' ', '\t']);
            let leading_closes = tokens[token_index..]
                .iter()
                .take_while(|token| {
                    (token.span().start as usize) < end
                        && matches!(token.ty, TokenType::CloseParen(_))
                })
                .count();
            if !content.trim().is_empty() {
                output.push_str(&"  ".repeat(depth.saturating_sub(leading_closes)));
                output.push_str(content);
            } else if line.ends_with('\n') {
                output.push('\n');
            }
        }
        while let Some(token) = tokens
            .get(token_index)
            .filter(|token| (token.span().start as usize) < end)
        {
            match token.ty {
                TokenType::OpenParen(..) => depth += 1,
                TokenType::CloseParen(..) => depth = depth.saturating_sub(1),
                _ => {},
            }
            token_index += 1;
        }
        anyhow::ensure!(
            output.len() <= crate::MAX_SOURCE_BYTES,
            "formatted source exceeds 8 MiB"
        );
        offset = end;
    }
    output.truncate(output.trim_end_matches('\n').len());
    output.push('\n');
    Ok(output)
}

#[cfg(test)]
#[path = "unit/format.rs"]
mod tests;
