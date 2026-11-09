//! Strict, single-pass path and query decoding. Segment boundaries never change.
use http::StatusCode;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Normalized {
    pub path: String,
    pub segments: Vec<String>,
    pub query: Vec<(String, String)>,
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode(text: &str, form: bool) -> Result<String, StatusCode> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        let b = match b {
            b'%' => hex(bytes.next().ok_or(StatusCode::BAD_REQUEST)?)
                .zip(hex(bytes.next().ok_or(StatusCode::BAD_REQUEST)?))
                .map(|(a, b)| a * 16 + b)
                .ok_or(StatusCode::BAD_REQUEST)?,
            b'+' if form => b' ',
            b => b,
        };
        if b == 0 || (!form && matches!(b, b'/' | b'\\')) {
            return Err(StatusCode::BAD_REQUEST);
        }
        decoded.push(b);
    }
    String::from_utf8(decoded).map_err(|_| StatusCode::BAD_REQUEST)
}

pub(crate) fn normalize(path: &str, query: Option<&str>) -> Result<Normalized, StatusCode> {
    if !path.starts_with('/')
        || path.len() > 2048
        || path.contains(['\0', '\\'])
        || path.contains("//")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let parts: Vec<_> = path[1..].split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() > 32 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let segments = parts
        .into_iter()
        .map(|s| {
            let s = decode(s, false)?;
            if matches!(s.as_str(), "." | "..") {
                return Err(StatusCode::BAD_REQUEST);
            }
            Ok(s)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut normalized = format!("/{}", segments.join("/"));
    if path.ends_with('/') && normalized != "/" {
        normalized.push('/');
    }
    let mut pairs = Vec::new();
    if let Some(query) = query.filter(|s| !s.is_empty()) {
        if query.len() > 2048 {
            return Err(StatusCode::BAD_REQUEST);
        }
        for pair in query.split('&') {
            if pairs.len() == 32 {
                return Err(StatusCode::BAD_REQUEST);
            }
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            pairs.push((decode(key, true)?, decode(value, true)?));
        }
    }
    Ok(Normalized {
        path: normalized,
        segments,
        query: pairs,
    })
}

#[cfg(test)]
#[path = "unit/request.rs"]
mod tests;
