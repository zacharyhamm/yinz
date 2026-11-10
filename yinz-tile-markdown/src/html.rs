//! HTML for untrusted Markdown. Raw markup is text; images never fetch resources.
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};

/// Conservative browser URL policy. Entity-looking input, controls, whitespace,
/// backslashes and authority-relative URLs are refused rather than normalized twice.
pub fn safe_link(url: &str) -> bool {
    if url.is_empty()
        || url.starts_with("//")
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    let head = url.split(['/', '?', '#']).next().unwrap_or_default();
    if head.contains('&') {
        return false;
    }
    match head.split_once(':') {
        Some((scheme, _)) => matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "mailto"
        ),
        None => true,
    }
}

/// Render CommonMark without active raw HTML, remote images or unsafe links.
/// Callers accepting untrusted input should impose their own input/output budgets.
pub fn render_safe(source: &str) -> String {
    let mut events = Vec::new();
    let mut image_depth = 0usize;
    let mut links = Vec::new();
    for event in Parser::new_ext(source, Options::ENABLE_STRIKETHROUGH) {
        if image_depth > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => image_depth += 1,
                Event::End(TagEnd::Image) => image_depth -= 1,
                Event::Text(text)
                | Event::Code(text)
                | Event::Html(text)
                | Event::InlineHtml(text) => events.push(Event::Text(text)),
                Event::SoftBreak | Event::HardBreak => events.push(Event::Text(" ".into())),
                _ => {},
            }
            continue;
        }
        match event {
            Event::Html(text) | Event::InlineHtml(text) => events.push(Event::Text(text)),
            Event::Start(Tag::Image { .. }) => image_depth = 1,
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                let safe = safe_link(&dest_url);
                links.push(safe);
                if safe {
                    let mut opening = String::from("<a href=\"");
                    escape(&mut opening, &dest_url);
                    opening.push_str("\" rel=\"noopener noreferrer\" title=\"");
                    escape(&mut opening, &title);
                    opening.push_str("\">");
                    events.push(Event::InlineHtml(CowStr::from(opening)));
                }
            },
            Event::End(TagEnd::Link) => {
                if links.pop() == Some(true) {
                    events.push(Event::InlineHtml("</a>".into()));
                }
            },
            other => events.push(other),
        }
    }
    let mut output = String::new();
    html::push_html(&mut output, events.into_iter());
    output
}
fn escape(output: &mut String, source: &str) {
    for c in source.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            c => output.push(c),
        }
    }
}
#[cfg(test)]
#[path = "unit/html.rs"]
mod tests;
