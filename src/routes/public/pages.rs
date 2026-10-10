use crate::templates::context::Crumb;

/// Cumulative breadcrumbs for a "/"-joined page path.
/// "a/b/c" -> [{a,/a}, {b,/a/b}, {c,/a/b/c}]
pub fn breadcrumbs(path: &str) -> Vec<Crumb> {
    let mut href = String::new();
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            href.push('/');
            href.push_str(seg);
            Crumb {
                label: seg.to_string(),
                href: href.clone(),
            }
        })
        .collect()
}
