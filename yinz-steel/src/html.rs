//! Opaque immutable markup. Untrusted JSON can never impersonate a node.
use std::collections::BTreeMap;
use steel::{
    rvals::{Custom, IntoSteelVal, Result, SteelVal},
    steel_vm::{engine::Engine, register_fn::RegisterFn},
};
const MAX: usize = 4 * 1024 * 1024;
const DEPTH: usize = 64;
#[derive(Clone)]
struct HtmlNode {
    markup: String,
    depth: usize,
}
impl Custom for HtmlNode {}
#[derive(Clone)]
struct TrustedHtml(String);
impl Custom for TrustedHtml {}
fn error(message: impl std::fmt::Display) -> steel::rerrs::SteelErr {
    crate::scheme_error(message)
}
fn string(value: &SteelVal) -> Result<&str> {
    match value {
        SteelVal::StringV(s) => Ok(s.as_str()),
        _ => Err(error("expected a string")),
    }
}
fn append(out: &mut String, text: &str) -> Result<()> {
    if out.len().saturating_add(text.len()) > MAX {
        return Err(error("HTML exceeds 4 MiB"));
    }
    out.push_str(text);
    Ok(())
}
fn escaped(out: &mut String, text: &str) -> Result<()> {
    for c in text.chars() {
        match c {
            '&' => append(out, "&amp;")?,
            '<' => append(out, "&lt;")?,
            '>' => append(out, "&gt;")?,
            '"' => append(out, "&quot;")?,
            '\'' => append(out, "&#39;")?,
            c => append(out, c.encode_utf8(&mut [0; 4]))?,
        }
    }
    Ok(())
}
fn children(out: &mut String, value: &SteelVal, depth: usize) -> Result<usize> {
    if depth > DEPTH {
        return Err(error("HTML depth exceeds 64"));
    }
    let mut max = depth;
    match value {
        SteelVal::StringV(s) => escaped(out, s.as_str())?,
        SteelVal::IntV(n) => escaped(out, &n.to_string())?,
        SteelVal::NumV(n) if n.is_finite() => escaped(out, &n.to_string())?,
        SteelVal::BigNum(n) if n.bits() <= (MAX * 3) as u64 => escaped(out, &n.to_string())?,
        SteelVal::BoolV(false) => {},
        SteelVal::SymbolV(s) if s.as_str() == "json-null" => {},
        SteelVal::ListV(list) => {
            for child in list.iter() {
                max = max.max(children(out, child, depth + 1)?);
            }
        },
        SteelVal::Custom(v) => {
            let v = v.read();
            if let Some(node) = v.as_any_ref().downcast_ref::<HtmlNode>() {
                max = depth.saturating_add(node.depth);
                if max > DEPTH {
                    return Err(error("HTML depth exceeds 64"));
                }
                append(out, &node.markup)?;
            } else if let Some(raw) = v.as_any_ref().downcast_ref::<TrustedHtml>() {
                append(out, &raw.0)?;
            } else {
                return Err(error("unsupported HTML child"));
            }
        },
        _ => {
            return Err(error(
                "HTML children must be text, numbers, fragments or native markup",
            ))
        },
    }
    Ok(max)
}
fn name(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
pub(crate) fn root_url(s: &str) -> bool {
    s.starts_with('/')
        && !s.starts_with("//")
        && !s
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        && !s.as_bytes().windows(3).any(|w| {
            w[0] == b'%'
                && std::str::from_utf8(&w[1..])
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
                    .is_some_and(|b| b.is_ascii_control() || matches!(b, b'/' | b'\\'))
        })
}
fn url(s: &str) -> bool {
    if s != s.trim() || !yinz_tile_markdown::html::safe_link(s) {
        return false;
    }
    root_url(s)
        || s.starts_with('#')
        || s.to_ascii_lowercase().starts_with("https:")
        || s.to_ascii_lowercase().starts_with("mailto:")
}
fn boolean(s: &str) -> bool {
    matches!(
        s,
        "disabled"
            | "checked"
            | "selected"
            | "multiple"
            | "required"
            | "readonly"
            | "hidden"
            | "open"
            | "autofocus"
            | "novalidate"
            | "reversed"
    )
}
fn attribute(tag: &str, key: &str) -> bool {
    if !name(key) || key.starts_with("on") || matches!(key, "style" | "srcdoc") {
        return false;
    }
    if tag == "script" {
        return matches!(key, "src" | "type");
    }
    matches!(
        key,
        "id" | "class" | "title" | "lang" | "dir" | "role" | "tabindex" | "hidden"
    ) || key.starts_with("data-")
        || key.starts_with("aria-")
        || match tag {
            "a" => matches!(key, "href" | "rel" | "target"),
            "img" => matches!(key, "src" | "alt" | "width" | "height" | "loading"),
            "link" => matches!(key, "href" | "rel" | "type"),
            "meta" => matches!(key, "charset" | "name" | "content"),
            "form" => matches!(key, "action" | "method" | "autocomplete" | "novalidate"),
            "input" => matches!(
                key,
                "type"
                    | "name"
                    | "value"
                    | "placeholder"
                    | "autocomplete"
                    | "disabled"
                    | "checked"
                    | "required"
                    | "readonly"
                    | "min"
                    | "max"
                    | "step"
                    | "minlength"
                    | "maxlength"
                    | "multiple"
                    | "autofocus"
            ),
            "button" => matches!(key, "type" | "name" | "value" | "disabled"),
            "label" => key == "for",
            "select" => matches!(key, "name" | "disabled" | "required" | "multiple"),
            "option" => matches!(key, "value" | "selected" | "disabled"),
            "textarea" => matches!(
                key,
                "name"
                    | "rows"
                    | "cols"
                    | "placeholder"
                    | "required"
                    | "readonly"
                    | "disabled"
                    | "maxlength"
            ),
            "th" | "td" => matches!(key, "colspan" | "rowspan" | "scope" | "headers"),
            "ol" => matches!(key, "start" | "reversed"),
            "time" => key == "datetime",
            "details" => key == "open",
            "progress" | "meter" => matches!(key, "value" | "min" | "max"),
            "fieldset" => key == "disabled",
            _ => false,
        }
}
fn node(args: &[SteelVal], form: bool) -> Result<SteelVal> {
    if args.len() < 2 {
        return Err(error("yinz-h expects tag, attributes and children"));
    }
    let tag = string(&args[0])?;
    if !(matches!(
        tag,
        "html"
            | "head"
            | "body"
            | "title"
            | "meta"
            | "link"
            | "script"
            | "noscript"
            | "div"
            | "span"
            | "p"
            | "a"
            | "img"
            | "br"
            | "hr"
            | "main"
            | "header"
            | "footer"
            | "nav"
            | "section"
            | "article"
            | "aside"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "pre"
            | "code"
            | "kbd"
            | "samp"
            | "blockquote"
            | "strong"
            | "em"
            | "b"
            | "i"
            | "s"
            | "small"
            | "sub"
            | "sup"
            | "mark"
            | "table"
            | "caption"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "button"
            | "input"
            | "label"
            | "select"
            | "option"
            | "textarea"
            | "fieldset"
            | "legend"
            | "details"
            | "summary"
            | "time"
            | "progress"
            | "meter"
    ) || (form && tag == "form"))
    {
        return Err(error("unsupported HTML tag"));
    }
    let SteelVal::HashMapV(attrs) = &args[1] else {
        return Err(error("HTML attributes must be a hash"));
    };
    let mut values = BTreeMap::new();
    let mut remaining = MAX;
    for (key, value) in attrs.iter() {
        let key = string(key)?;
        if !attribute(tag, key) {
            return Err(error(format!("unsupported HTML attribute: {key}")));
        }
        let bytes = key.len().saturating_add(match value {
            SteelVal::StringV(text) => text.len(),
            SteelVal::BigNum(n) => usize::try_from(n.bits()).unwrap_or(usize::MAX),
            _ => 32,
        });
        remaining = remaining
            .checked_sub(bytes)
            .ok_or_else(|| error("HTML attributes exceed 4 MiB"))?;
        match value {
            SteelVal::BoolV(false) => continue,
            SteelVal::SymbolV(s) if s.as_str() == "json-null" => continue,
            SteelVal::BoolV(true) if boolean(key) => {
                values.insert(key.to_owned(), None);
            },
            SteelVal::StringV(s) => {
                if s.len() > MAX {
                    return Err(error("HTML exceeds 4 MiB"));
                }
                values.insert(key.to_owned(), Some(s.to_string()));
            },
            SteelVal::IntV(n) => {
                values.insert(key.to_owned(), Some(n.to_string()));
            },
            SteelVal::NumV(n) if n.is_finite() => {
                values.insert(key.to_owned(), Some(n.to_string()));
            },
            SteelVal::BigNum(n) => {
                values.insert(key.to_owned(), Some(n.to_string()));
            },
            _ => return Err(error("invalid HTML attribute value")),
        }
    }
    if form {
        if values
            .get("method")
            .is_some_and(|v| v.as_deref() != Some("post"))
        {
            return Err(error("form method is fixed to post"));
        }
        values.insert("method".into(), Some("post".into()));
    }
    for (key, value) in &values {
        if matches!(key.as_str(), "href" | "src" | "action") {
            let s = value.as_deref().ok_or_else(|| error("URL must be text"))?;
            if !if tag == "a" { url(s) } else { root_url(s) } {
                return Err(error("unsafe HTML URL"));
            }
        }
    }
    if tag == "form" && !values.contains_key("action") {
        return Err(error("form needs a root-relative action"));
    }
    if tag == "link"
        && (!values.contains_key("href")
            || !matches!(
                values.get("rel").and_then(|v| v.as_deref()),
                Some("stylesheet" | "icon" | "manifest")
            ))
    {
        return Err(error("unsupported link resource"));
    }
    if tag == "script"
        && (!values.contains_key("src")
            || !matches!(
                values.get("type").and_then(|v| v.as_deref()),
                None | Some("module" | "text/javascript")
            ))
    {
        return Err(error("scripts require external same-origin src"));
    }
    let mut content = String::new();
    let mut depth = 0;
    for child in &args[2..] {
        depth = depth.max(children(&mut content, child, 1)?);
    }
    let void = matches!(tag, "meta" | "link" | "img" | "br" | "hr" | "input");
    if (void || tag == "script") && !content.is_empty() {
        return Err(error("script/void elements refuse children"));
    }
    let mut markup = String::new();
    append(&mut markup, "<")?;
    append(&mut markup, tag)?;
    for (key, value) in values {
        append(&mut markup, " ")?;
        append(&mut markup, &key)?;
        if let Some(value) = value {
            append(&mut markup, "=\"")?;
            escaped(&mut markup, &value)?;
            append(&mut markup, "\"")?;
        }
    }
    append(&mut markup, ">")?;
    append(&mut markup, &content)?;
    if !void {
        append(&mut markup, "</")?;
        append(&mut markup, tag)?;
        append(&mut markup, ">")?;
    }
    HtmlNode {
        markup,
        depth: depth.max(1),
    }
    .into_steelval()
}
fn render(args: &[SteelVal]) -> Result<SteelVal> {
    let mut out = String::new();
    for value in args {
        children(&mut out, value, 0)?;
    }
    Ok(SteelVal::StringV(out.into()))
}
fn document(args: &[SteelVal]) -> Result<SteelVal> {
    if !(2..=3).contains(&args.len()) {
        return Err(error(
            "document expects head, body and optional html attributes",
        ));
    }
    let attrs = if let Some(value) = args.get(2) {
        value.clone()
    } else {
        crate::to_scheme(&serde_json::json!({"lang":"en"})).map_err(error)?
    };
    let empty = crate::to_scheme(&serde_json::json!({})).map_err(error)?;
    let head = node(
        &[
            SteelVal::StringV("head".into()),
            empty.clone(),
            args[0].clone(),
        ],
        false,
    )?;
    let body = node(
        &[SteelVal::StringV("body".into()), empty, args[1].clone()],
        false,
    )?;
    let html = node(
        &[SteelVal::StringV("html".into()), attrs, head, body],
        false,
    )?;
    let mut markup = "<!doctype html>".to_string();
    let depth = children(&mut markup, &html, 0)?;
    HtmlNode { markup, depth }.into_steelval()
}
// Bound JSON input before the bridge copies it; escapes are bounded again at emission.
fn json_budget(value: &SteelVal, depth: usize, remaining: &mut usize) -> Result<()> {
    if depth > DEPTH {
        return Err(error("JSON script depth exceeds 64"));
    }
    let size = match value {
        SteelVal::StringV(s) => s.len().saturating_add(2),
        _ => 32,
    };
    *remaining = remaining
        .checked_sub(size)
        .ok_or_else(|| error("JSON script exceeds 4 MiB"))?;
    match value {
        SteelVal::ListV(values) => {
            for v in values.iter() {
                json_budget(v, depth + 1, remaining)?;
            }
        },
        SteelVal::HashMapV(values) => {
            for (k, v) in values.iter() {
                json_budget(k, depth + 1, remaining)?;
                json_budget(v, depth + 1, remaining)?;
            }
        },
        _ => {},
    }
    Ok(())
}
struct JsonBuffer(Vec<u8>);
impl std::io::Write for JsonBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX {
            return Err(std::io::Error::other("JSON script exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn register(engine: &mut Engine) {
    engine.register_value("yinz-h", SteelVal::FuncV(|args| node(args, false)));
    engine.register_value(
        "yinz-html-form",
        SteelVal::FuncV(|args| {
            let mut all = vec![SteelVal::StringV("form".into())];
            all.extend_from_slice(args);
            node(&all, true)
        }),
    );
    engine.register_value("yinz-html", SteelVal::FuncV(render));
    engine.register_value("yinz-html-document", SteelVal::FuncV(document));
    engine.register_fn("yinz-html-escape", |value: SteelVal| -> Result<String> {
        let s = string(&value)?;
        let mut out = String::new();
        escaped(&mut out, s)?;
        Ok(out)
    });
    engine.register_fn("yinz-html-raw", |value: SteelVal| -> Result<TrustedHtml> {
        let s = string(&value)?;
        if s.len() > MAX {
            return Err(error("HTML exceeds 4 MiB"));
        }
        Ok(TrustedHtml(s.to_owned()))
    });
    engine.register_fn(
        "yinz-html-json-script",
        |id: SteelVal, value: SteelVal| -> Result<TrustedHtml> {
            let id = string(&id)?;
            let mut remaining = MAX;
            json_budget(&value, 0, &mut remaining)?;
            let value = crate::from_scheme(&value).map_err(error)?;
            let mut encoded = JsonBuffer(Vec::new());
            serde_json::to_writer(&mut encoded, &value).map_err(error)?;
            let encoded = String::from_utf8(encoded.0).map_err(error)?;
            let mut out = String::from("<script type=\"application/json\" id=\"");
            escaped(&mut out, id)?;
            append(&mut out, "\">")?;
            for c in encoded.chars() {
                match c {
                    '<' => append(&mut out, "\\u003c")?,
                    '>' => append(&mut out, "\\u003e")?,
                    '&' => append(&mut out, "\\u0026")?,
                    '\u{2028}' => append(&mut out, "\\u2028")?,
                    '\u{2029}' => append(&mut out, "\\u2029")?,
                    c => append(&mut out, c.encode_utf8(&mut [0; 4]))?,
                }
            }
            append(&mut out, "</script>")?;
            Ok(TrustedHtml(out))
        },
    );
    engine.register_fn(
        "yinz-markdown->html",
        |value: SteelVal| -> Result<TrustedHtml> {
            let s = string(&value)?;
            if s.len() > 1024 * 1024 {
                return Err(error("Markdown exceeds 1 MiB"));
            }
            let html = yinz_tile_markdown::html::render_safe(s);
            if html.len() > MAX {
                return Err(error("HTML exceeds 4 MiB"));
            }
            Ok(TrustedHtml(html))
        },
    );
}
