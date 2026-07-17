//! Markdown rendering with `<name ...>` directives.
//!
//! Block- and inline-level directives use an HTML-tag syntax,
//! `<name attr="value" ...>`. Each directive resolves to HTML (or, for `<page>`,
//! to markdown that is recursively re-scanned) and is spliced inline into the
//! document before the markdown parser runs.
//!
//! ```text
//! <page path="infra/desktop/syncthing">
//! <page id="7">
//! <file path="spec.pdf">
//! <file id="42">
//! <file hash="ab12...">
//! <image path="diagram.png" alt="Architecture">
//! <gallery id="3">
//! <gallery path="holiday-2024">
//! <fen path="opening.fen" size="large">
//! <pgn hash="ab12..." move="12" size="small">
//! <fen>rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1</fen>
//! <pgn move="10">1. e4 e5 2. Nf3 Nc6 ...</pgn>
//! <mermaid path="diagrams/flow.mmd" theme="dark">
//! <mermaid theme="forest">graph TD
//!   A[Start] --> B[End]</mermaid>
//! <json path="data/stats.json" query=".rows[]" type="table">
//! <json query=".rows[]" type="table">{ "rows": [{"a": 1}] }</json>
//! ```
//!
//! Attribute values may be double-quoted, single-quoted, or unquoted (no
//! whitespace). `<fen>`/`<pgn>` also accept a paired-tag form whose body holds
//! the position/game inline (multiple lines allowed); a body makes
//! `path`/`id`/`hash` optional.
//!
//! Lookup keys: exactly one of
//! - file-based (`<file>`/`<image>`/`<fen>`/`<pgn>`/`<mermaid>`/`<json>`): `path`, `id`, or `hash` (sha256)
//! - `<gallery>`: `path` or `id`
//! - `<page>`: `path` or `id`
//!
//! Only an allow-list of names is treated as directives; any other `<tag>`
//! (including a real `<img>`) passes through as raw HTML. Directives inside
//! fenced code blocks (` ``` `, `~~~`) and inline code spans (`` ` ``) are
//! passed through verbatim — *except* a fence whose info string is `mermaid`,
//! which renders as a diagram just like `<mermaid>`.
//!
//! Each directive's rendered HTML is spliced inline (wrapped in blank lines so
//! it forms a raw-HTML block) and passes through the markdown parser verbatim.
//!
//! Module layout: [`directives`] parses the `<name ...>` tag syntax into a
//! [`Directive`]; [`renderer`] walks the document dispatching directives;
//! [`handlers`] implements each directive family; [`lookup`] resolves
//! file/gallery/page arguments to DB rows; [`highlight`] does server-side
//! syntax highlighting of fenced code blocks; [`links`] rewrites internal
//! markdown links.

/// Human-readable summary of the custom markdown directives. Shared by the MCP
/// server instructions and the assistant system prompt so AI tools know the
/// exact syntax they should produce.
pub const MARKDOWN_EXTENSIONS_DOC: &str = "\
Directives use an HTML-tag syntax `<name attr=\"value\">`. Only the names below
are directives; any other `<tag>` is passed through as raw HTML. Lookup keys:
- file-based (`<file>`/`<image>`/`<fen>`/`<pgn>`/`<mermaid>`/`<json>`): exactly one of `path`, `id`, or `hash` (sha256)
- `<gallery>`: exactly one of `path` or `id`
- `<page>`: exactly one of `path` or `id`

- `<page path=\"section/sub/page\">` / `<page id=\"N\">` — transclude another page's rendered content inline.
- `<file path=\"...\">` — embeds an image (if mime image/*) or a download link otherwise.
- `<image path=\"...\" alt=\"...\">` — force image embed (with link to full size and caption).
- `<gallery path=\"...\">` — embeds a gallery grid of thumbnails.
- `<fen path=\"...\" size=\"small|large\">` — static chess board from a stored .fen file (`sm`/`lg` accepted as aliases).
- `<fen>FEN string</fen>` — static chess board from an inline FEN position.
- `<pgn path=\"...\" move=\"N\" size=\"small|large\">` — playable game from a stored .pgn file.
- `<pgn move=\"N\">PGN moves</pgn>` — playable game from an inline PGN (multiple lines allowed).
- `<mermaid path=\"...\" theme=\"default|dark|forest|neutral\" size=\"small|large\">` — diagram rendered to SVG from a stored Mermaid file.
- `<mermaid theme=\"...\">DIAGRAM</mermaid>` — diagram rendered to SVG from an inline Mermaid definition (multiple lines allowed).
- A fenced code block with info string `mermaid` also renders as a diagram, same as `<mermaid>`.
- `<json path=\"...\" query=\".rows[]\" type=\"table\">` — run a jq query over a JSON file blob (or inline `<json query=\"...\">{...}</json>`) and render the result; `type=\"table\"` builds an HTML table.
- Internal links `[Text](Path/To/Page.md)` are auto-rewritten to lowercase absolute paths.";

mod directives;
mod handlers;
mod highlight;
mod links;
mod lookup;
mod renderer;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use minijinja::Environment;
use pulldown_cmark::{Options, Parser, html};
use sea_orm::DatabaseConnection;

struct RenderCtx<'a> {
    db: &'a DatabaseConnection,
    tmpl: &'a Environment<'static>,
    logged_in: bool,
    /// Pages already on the transclusion stack — prevents infinite recursion.
    visited_pages: HashSet<String>,
}

pub async fn render(
    md: &str,
    db: &DatabaseConnection,
    tmpl: &Environment<'static>,
    logged_in: bool,
) -> String {
    let mut ctx = RenderCtx {
        db,
        tmpl,
        logged_in,
        visited_pages: HashSet::new(),
    };

    let expanded = renderer::expand_directives(md, &mut ctx).await;

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&expanded, opts);
    let events = highlight::highlight_code_block_events(parser.collect());
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());

    links::rewrite_internal_links(&out)
}
