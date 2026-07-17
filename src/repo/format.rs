//! Text formatters shared by every surface that renders repo data as plain
//! text for a tool caller — the MCP server (`src/routes/mcp.rs`) and the AI
//! assistant's built-in tools (`src/ai/tools/*.rs`). Kept here, next to the
//! repo functions that produce the data, so the two edges can't drift again
//! (see issue #25).

use crate::entity::{page, tag};
use crate::repo::pages_search::SearchResult;

/// Render a single page: `# path` header, optional `Tags:`/`Summary:` lines,
/// always `Modified:`, optional `Private: yes`, then a `---` separator and
/// the raw markdown.
pub fn format_page(page: &page::Model, tag_names: &[String]) -> String {
    let mut out = format!("# {}\n\n", page.path);
    if !tag_names.is_empty() {
        out.push_str(&format!("Tags: {}\n", tag_names.join(", ")));
    }
    if let Some(summary) = &page.summary {
        out.push_str(&format!("Summary: {summary}\n"));
    }
    out.push_str(&format!("Modified: {}\n", page.modified_at));
    if page.private {
        out.push_str("Private: yes\n");
    }
    out.push_str("\n---\n");
    out.push_str(&page.markdown);
    out
}

/// Render a page search result: one `path: summary` line per match (just
/// `path` when there's no summary), then a `--- total: N, has_more: B[,
/// next_offset: M] ---` trailer for pagination.
pub fn format_search_results(result: &SearchResult, limit: u64, offset: u64) -> String {
    if result.total == 0 {
        return "No pages found.".to_string();
    }
    let has_more = offset + limit < result.total;
    let mut out = result
        .pages
        .iter()
        .map(|p| match &p.summary {
            Some(s) if !s.is_empty() => format!("{}: {s}", p.path),
            _ => p.path.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&format!(
        "\n\n--- total: {}, has_more: {has_more}",
        result.total
    ));
    if has_more {
        out.push_str(&format!(", next_offset: {}", offset + limit));
    }
    out.push_str(" ---");
    out
}

/// Render the full tag list: one `[id] name: description` line per tag
/// (`[id] name` when there's no description).
pub fn format_tags(tags: &[tag::Model]) -> String {
    if tags.is_empty() {
        return "No tags defined.".to_string();
    }
    tags.iter()
        .map(|t| match &t.description {
            Some(d) if !d.is_empty() => format!("[{}] {}: {d}", t.id, t.name),
            _ => format!("[{}] {}", t.id, t.name),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn sample_page(path: &str, summary: Option<&str>, private: bool) -> page::Model {
        let ts = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .unwrap();
        page::Model {
            id: 1,
            path: path.to_string(),
            summary: summary.map(String::from),
            markdown: "content".to_string(),
            tag_ids: vec![],
            private,
            created_at: ts,
            created_by: 1,
            modified_at: ts,
            modified_by: 1,
        }
    }

    fn sample_tag(id: i32, name: &str, description: Option<&str>) -> tag::Model {
        tag::Model {
            id,
            name: name.to_string(),
            description: description.map(String::from),
        }
    }

    #[test]
    fn format_page_includes_tags_summary_modified_and_private() {
        let page = sample_page("obsidian/rust", Some("notes"), true);
        let out = format_page(&page, &["Rust".to_string(), "Notes".to_string()]);
        assert!(out.starts_with("# obsidian/rust\n\n"));
        assert!(out.contains("Tags: Rust, Notes\n"));
        assert!(out.contains("Summary: notes\n"));
        assert!(out.contains("Modified: 2026-01-02"));
        assert!(out.contains("Private: yes\n"));
        assert!(out.ends_with("\n---\ncontent"));
    }

    #[test]
    fn format_page_omits_empty_fields() {
        let page = sample_page("about", None, false);
        let out = format_page(&page, &[]);
        assert!(!out.contains("Tags:"));
        assert!(!out.contains("Summary:"));
        assert!(!out.contains("Private:"));
        assert!(out.contains("Modified:"));
    }

    #[test]
    fn format_search_results_empty() {
        let result = SearchResult {
            pages: vec![],
            total: 0,
        };
        assert_eq!(format_search_results(&result, 20, 0), "No pages found.");
    }

    #[test]
    fn format_search_results_paginates() {
        let result = SearchResult {
            pages: vec![
                sample_page("a", Some("first"), false),
                sample_page("b", None, false),
            ],
            total: 5,
        };
        let out = format_search_results(&result, 2, 0);
        assert!(out.contains("a: first\nb"));
        assert!(out.contains("--- total: 5, has_more: true, next_offset: 2 ---"));
    }

    #[test]
    fn format_search_results_last_page_has_no_next_offset() {
        let result = SearchResult {
            pages: vec![sample_page("a", None, false)],
            total: 1,
        };
        let out = format_search_results(&result, 20, 0);
        assert!(out.ends_with("--- total: 1, has_more: false ---"));
    }

    #[test]
    fn format_tags_lists_with_and_without_description() {
        let tags = vec![
            sample_tag(1, "Rust", Some("systems language")),
            sample_tag(2, "Empty", None),
        ];
        let out = format_tags(&tags);
        assert_eq!(out, "[1] Rust: systems language\n[2] Empty");
    }

    #[test]
    fn format_tags_empty_list() {
        assert_eq!(format_tags(&[]), "No tags defined.");
    }
}
