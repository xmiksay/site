//! Directive tag parsing: recognizing `<name ...>` against the allow-list and
//! parsing its attributes.

use std::collections::HashMap;

pub(super) struct Directive {
    /// Internal handler name (`page`/`file`/`img`/`gallery`/`fen`/`pgn`/`mermaid`).
    pub(super) name: String,
    pub(super) args: HashMap<String, String>,
    /// Body of a paired container tag (`<fen>…</fen>`, `<pgn>…</pgn>`); `None`
    /// for void directives.
    pub(super) inner: Option<String>,
}

impl Directive {
    pub(super) fn arg(&self, key: &str) -> Option<&str> {
        self.args.get(key).map(String::as_str)
    }
}

/// Surface names recognized as directives in tag form. Everything else (incl. a
/// real `<img>`) is left untouched as raw HTML.
const DIRECTIVE_NAMES: [&str; 8] = [
    "page", "file", "image", "gallery", "fen", "pgn", "mermaid", "json",
];

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// Parse a directive tag at the start of `s` (which must begin with `<`).
/// Returns the directive and the number of bytes consumed (through `>` / `/>`).
/// The surface name `image` is canonicalized to the handler name `img`.
pub(super) fn parse_tag_directive(s: &str) -> Option<(Directive, usize)> {
    let after_lt = s.strip_prefix('<')?;
    // Closing tags are not directives.
    if after_lt.starts_with('/') {
        return None;
    }
    let name_end = after_lt
        .find(|c: char| !is_name_char(c))
        .unwrap_or(after_lt.len());
    let name = &after_lt[..name_end];
    if name.is_empty() || !DIRECTIVE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        return None;
    }

    let mut rest = &after_lt[name_end..];
    let mut args: HashMap<String, String> = HashMap::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None; // unterminated tag
        }
        if let Some(after) = rest.strip_prefix("/>") {
            rest = after;
            break;
        }
        if let Some(after) = rest.strip_prefix('>') {
            rest = after;
            break;
        }
        let key_end = rest
            .find(|c: char| c.is_whitespace() || c == '=' || c == '>' || c == '/')
            .unwrap_or(rest.len());
        if key_end == 0 {
            return None; // malformed attribute
        }
        let key = rest[..key_end].to_string();
        rest = &rest[key_end..];
        let after_key = rest.trim_start();
        if let Some(after_eq) = after_key.strip_prefix('=') {
            let (value, remainder) = parse_tag_value(after_eq.trim_start())?;
            args.insert(key, value);
            rest = remainder;
        } else {
            args.insert(key, String::new()); // bare flag
        }
    }

    let canonical = match name.to_ascii_lowercase().as_str() {
        "image" => "img".to_string(),
        other => other.to_string(),
    };

    let consumed = s.len() - rest.len();
    Some((
        Directive {
            name: canonical,
            args,
            inner: None,
        },
        consumed,
    ))
}

/// Parse a double-quoted, single-quoted, or unquoted attribute value. Returns
/// `(value, remainder)`. Unquoted values stop at whitespace or `>` — never at
/// `/`, so paths keep their slashes.
fn parse_tag_value(s: &str) -> Option<(String, &str)> {
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        Some((unescape_tag(&rest[..end]), &rest[end + 1..]))
    } else if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'')?;
        Some((unescape_tag(&rest[..end]), &rest[end + 1..]))
    } else {
        let end = s
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(s.len());
        if end == 0 {
            return None;
        }
        Some((unescape_tag(&s[..end]), &s[end..]))
    }
}

fn unescape_tag(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
