//! Strict smoke render: render every page template and directive partial of
//! a design against the site's real data with undefined values as errors,
//! collecting every failure instead of stopping at the first. Validates a
//! design (or a draft) before it goes live.

use std::collections::BTreeSet;

use anyhow::Context as _;
use minijinja::Environment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::Serialize;

use super::context::{Layout, PageSearchContext, PageView, TagView};
use super::contract::{TEMPLATES, TemplateKind};
use super::{DesignLoader, samples, strict_environment};
use crate::entity::{menu, page, tag};
use crate::markdown;
use crate::repo::pages_search;
use crate::routes::build_menu;
use crate::routes::public::{menu_context, page_context, search::page_window};
use crate::storage::Storage;

/// One template failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SmokeError {
    /// The template the error is in (`base.html`, `markdown/fen.html`, …).
    pub template: String,
    /// 1-based line in that template, when known.
    pub line: Option<usize>,
    pub message: String,
    /// The render that hit it first, e.g. ``page `notes/rust` ``.
    pub case: String,
}

#[derive(Debug, Default, Serialize)]
pub struct SmokeReport {
    /// Every render performed, in order.
    pub cases: Vec<String>,
    /// Distinct failures (same template, line and message reported once).
    pub errors: Vec<SmokeError>,
}

impl SmokeReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Text that marks a page as exercising a directive partial.
const DIRECTIVE_MARKERS: [&str; 9] = [
    "<page",
    "<file",
    "<image",
    "<gallery",
    "<fen",
    "<pgn",
    "<mermaid",
    "```mermaid",
    "<json",
];

/// Smoke-render the design `load` resolves (`templates/…` → bytes): 404 and
/// `base.html`, the home menu item, the newest page plus the first page using
/// each directive, and search with and without a tag — each anonymous and
/// logged in. Partials no real page reaches, and page shapes the data lacks
/// (no menu item, no page), render with the contract's example contexts.
///
/// `Err` only for a database failure; template failures are in the report.
pub async fn smoke_render(
    db: &DatabaseConnection,
    storage: &Storage,
    load: impl DesignLoader,
) -> anyhow::Result<SmokeReport> {
    let mut run = Run {
        env: strict_environment(load),
        report: SmokeReport::default(),
        seen: BTreeSet::new(),
    };
    let base = Layout::new(build_menu(db, true).await, true);
    let layout = |logged_in| Layout {
        logged_in,
        ..base.clone()
    };

    for logged_in in [false, true] {
        run.page("404.html", "404", logged_in, &layout(logged_in));
        run.page("base.html", "layout", logged_in, &layout(logged_in));
    }

    match home_menu(db).await? {
        Some(m) => {
            let case = format!("menu `/{}`", m.path);
            let body = run.markdown(db, storage, &m.markdown, &case).await;
            for logged_in in [false, true] {
                let ctx = menu_context(layout(logged_in), &m, body.clone());
                run.page("path_page.html", &case, logged_in, &ctx);
            }
        }
        None => {
            for logged_in in [false, true] {
                let ctx = samples::path_page_menu(logged_in);
                run.page("path_page.html", "example menu item", logged_in, &ctx);
            }
        }
    }

    let pages = sample_pages(db).await?;
    for pg in &pages {
        let case = format!("page `{}`", pg.path);
        let body = run.markdown(db, storage, &pg.markdown, &case).await;
        for logged_in in [false, true] {
            let ctx = page_context(db, layout(logged_in), pg, body.clone()).await;
            run.page("path_page.html", &case, logged_in, &ctx);
        }
    }
    if pages.is_empty() {
        for logged_in in [false, true] {
            let ctx = samples::path_page_page(logged_in);
            run.page("path_page.html", "example page", logged_in, &ctx);
        }
    }

    let first_tag = tag::Entity::find()
        .order_by_asc(tag::Column::Id)
        .one(db)
        .await
        .context("smoke render: load a tag")?;
    for tag in [None, first_tag] {
        let ctx = search_context(db, layout(true), tag).await?;
        let case = match &ctx.tag {
            Some(t) => format!("search tag `{}`", t.name),
            None => "search".to_string(),
        };
        for logged_in in [false, true] {
            let ctx = PageSearchContext {
                layout: layout(logged_in),
                ..ctx.clone()
            };
            run.page("page_search.html", &case, logged_in, &ctx);
        }
    }

    for spec in TEMPLATES.iter().filter(|t| t.kind == TemplateKind::Partial) {
        if !run.seen.contains(spec.name) {
            for sample in spec.samples() {
                run.render(spec.name, "example context", &sample);
            }
        }
    }
    Ok(run.report)
}

struct Run {
    env: Environment<'static>,
    report: SmokeReport,
    /// Templates rendered so far.
    seen: BTreeSet<String>,
}

impl Run {
    fn page(&mut self, name: &str, case: &str, logged_in: bool, ctx: &impl Serialize) {
        let who = if logged_in { "logged in" } else { "anonymous" };
        self.render(name, &format!("{case} ({who})"), ctx);
    }

    fn render(&mut self, name: &str, case: &str, ctx: &impl Serialize) {
        self.report.cases.push(format!("{name}: {case}"));
        self.seen.insert(name.to_string());
        let result = self.env.get_template(name).and_then(|t| t.render(ctx));
        if let Err(e) = result {
            self.error(name, case, &e);
        }
    }

    /// Render page markdown through the strict environment, keeping its
    /// partial failures; returns the body HTML for the page template.
    async fn markdown(
        &mut self,
        db: &DatabaseConnection,
        storage: &Storage,
        md: &str,
        case: &str,
    ) -> String {
        let (html, checks) = markdown::render_checked(md, db, storage, &self.env, true).await;
        for name in checks.rendered {
            self.report.cases.push(format!("{name}: {case}"));
            self.seen.insert(name);
        }
        for (name, e) in checks.errors {
            self.error(&name, case, &e);
        }
        html
    }

    fn error(&mut self, name: &str, case: &str, e: &minijinja::Error) {
        let error = SmokeError {
            template: e.name().unwrap_or(name).to_string(),
            line: e.line(),
            message: match e.detail() {
                Some(detail) => format!("{}: {detail}", e.kind()),
                None => e.kind().to_string(),
            },
            case: case.to_string(),
        };
        let duplicate = self.report.errors.iter().any(|known| {
            (&known.template, known.line, &known.message)
                == (&error.template, error.line, &error.message)
        });
        if !duplicate {
            self.report.errors.push(error);
        }
    }
}

/// The home menu item (path `""`), else the first one.
async fn home_menu(db: &DatabaseConnection) -> anyhow::Result<Option<menu::Model>> {
    let home = menu::Entity::find()
        .filter(menu::Column::Path.eq(""))
        .one(db)
        .await
        .context("smoke render: load the home menu item")?;
    if home.is_some() {
        return Ok(home);
    }
    menu::Entity::find()
        .order_by_asc(menu::Column::OrderIndex)
        .one(db)
        .await
        .context("smoke render: load a menu item")
}

/// The newest page plus the first page containing each directive marker.
async fn sample_pages(db: &DatabaseConnection) -> anyhow::Result<Vec<page::Model>> {
    let mut pages = Vec::new();
    let newest = page::Entity::find()
        .order_by_desc(page::Column::ModifiedAt)
        .one(db)
        .await
        .context("smoke render: load the newest page")?;
    pages.extend(newest);
    for marker in DIRECTIVE_MARKERS {
        let found = page::Entity::find()
            .filter(page::Column::Markdown.contains(marker))
            .order_by_asc(page::Column::Id)
            .one(db)
            .await
            .with_context(|| format!("smoke render: find a page using {marker}"))?;
        if let Some(pg) = found
            && !pages.iter().any(|p: &page::Model| p.id == pg.id)
        {
            pages.push(pg);
        }
    }
    Ok(pages)
}

/// Search as `/search` builds it: a second result page of one result (so
/// both pagination links show when there are enough pages), optionally
/// filtered by `tag`.
async fn search_context(
    db: &DatabaseConnection,
    layout: Layout,
    tag: Option<tag::Model>,
) -> anyhow::Result<PageSearchContext> {
    let (limit, offset) = (1, 1);
    let tag_name = tag.as_ref().map(|t| t.name.clone());
    let result = pages_search::search(db, None, tag_name.as_deref(), None, true, limit, offset)
        .await
        .map_err(|e| anyhow::anyhow!("smoke render: search: {e}"))?;
    let (prev_offset, next_offset) = page_window(offset, limit, result.total);
    Ok(PageSearchContext {
        layout,
        q: String::new(),
        tag_name: tag_name.unwrap_or_default(),
        tag: tag.map(TagView::from),
        path_prefix: String::new(),
        pages: result.pages.iter().map(PageView::from).collect(),
        total: result.total,
        limit,
        offset,
        prev_offset,
        next_offset,
    })
}
