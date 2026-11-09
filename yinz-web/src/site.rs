//! Conditional retrieval uses weak comparison for GET and HEAD.
use http::HeaderMap;

pub(crate) fn strong_etag(value: &str) -> bool {
    value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && value.as_bytes()[1..value.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
}
fn matches(mut list: &str, etag: &str) -> bool {
    if list.trim() == "*" {
        return true;
    }
    let mut found = false;
    loop {
        list = list.trim_start_matches([' ', '\t']);
        if let Some(rest) = list.strip_prefix("W/") {
            list = rest;
        }
        let Some(rest) = list.strip_prefix('"') else {
            return false;
        };
        let Some(end) = rest.find('"') else {
            return false;
        };
        let value = &list[..end + 2];
        if !strong_etag(value) {
            return false;
        }
        found |= value == etag;
        list = list[end + 2..].trim_start_matches([' ', '\t']);
        if list.is_empty() {
            return found;
        }
        let Some(rest) = list.strip_prefix(',') else {
            return false;
        };
        list = rest;
    }
}
pub(crate) fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all("if-none-match")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|list| matches(list, etag))
}

#[cfg(test)]
#[path = "unit/site.rs"]
mod tests;
