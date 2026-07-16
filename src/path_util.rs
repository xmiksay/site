/// Canonical form for any user-facing hierarchical path used as a row key
/// (pages, galleries, files, menu items). Trims whitespace, strips leading and
/// trailing slashes, collapses interior duplicate slashes, and lowercases.
///
/// The whisper / `paths/children` queries assume this canonical form, so all
/// writes must go through this function to stay searchable.
pub fn normalize(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut prev_slash = false;
    for ch in trimmed.chars() {
        if ch == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
            out.push('/');
        } else {
            prev_slash = false;
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

/// Same as [`normalize`] but normalizes a prefix used in a LIKE query: returns
/// either an empty string (root) or a value that ends with `/`.
pub fn normalize_prefix(raw: &str) -> String {
    let n = normalize(raw);
    if n.is_empty() { n } else { format!("{n}/") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_and_lowercases() {
        assert_eq!(normalize("  Foo/Bar  "), "foo/bar");
        assert_eq!(normalize("ABOUT"), "about");
    }

    #[test]
    fn normalize_strips_and_collapses_slashes() {
        assert_eq!(normalize("/a/b/"), "a/b");
        assert_eq!(normalize("a//b///c"), "a/b/c");
        assert_eq!(normalize("///"), "");
    }

    #[test]
    fn normalize_empty_and_whitespace_yield_root() {
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn normalize_preserves_non_ascii_lowercasing() {
        // Multi-char lowercase mappings (e.g. the folding of some Unicode
        // uppercase) must not be dropped — this is why normalize iterates
        // char::to_lowercase rather than calling str::to_lowercase piecemeal.
        assert_eq!(normalize("Č/Ř"), "č/ř");
    }

    #[test]
    fn normalize_prefix_appends_trailing_slash() {
        assert_eq!(normalize_prefix("obsidian/rust"), "obsidian/rust/");
        assert_eq!(normalize_prefix("/Notes/"), "notes/");
    }

    #[test]
    fn normalize_prefix_root_stays_empty() {
        assert_eq!(normalize_prefix(""), "");
        assert_eq!(normalize_prefix("///"), "");
    }
}
