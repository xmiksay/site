//! Internal links: rewrite relative href values to page paths.
//! `[Syncthing](Infra/Desktop/Syncthing.md)` → `<a href="/infra/desktop/syncthing">`

pub(super) fn rewrite_internal_links(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(pos) = rest.find("href=\"") {
        out.push_str(&rest[..pos + 6]);
        rest = &rest[pos + 6..];
        let Some(end) = rest.find('"') else { break };
        let href = &rest[..end];
        if is_internal_link(href) {
            let path = href.strip_suffix(".md").unwrap_or(href).to_lowercase();
            out.push('/');
            out.push_str(&path);
        } else {
            out.push_str(href);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn is_internal_link(href: &str) -> bool {
    !href.is_empty()
        && !href.contains("://")
        && !href.starts_with('/')
        && !href.starts_with('#')
        && !href.starts_with("mailto:")
}
