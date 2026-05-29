use crate::entity::page;

#[derive(serde::Serialize)]
pub struct Crumb {
    pub label: String,
    pub href: String,
}

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

#[derive(serde::Serialize)]
pub struct PageView {
    pub id: i32,
    pub path: String,
    pub summary: Option<String>,
    pub tag_ids: Vec<i32>,
    pub private: bool,
    pub created_at: String,
    pub modified_at: String,
}

impl From<&page::Model> for PageView {
    fn from(p: &page::Model) -> Self {
        Self {
            id: p.id,
            path: p.path.clone(),
            summary: p.summary.clone(),
            tag_ids: p.tag_ids.clone(),
            private: p.private,
            created_at: p.created_at.to_string(),
            modified_at: p.modified_at.to_string(),
        }
    }
}
