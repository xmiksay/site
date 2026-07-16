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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

use minijinja::{Environment, context};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

use crate::entity::file as file_entity;
use crate::entity::gallery as gallery_entity;
use crate::entity::page as page_entity;
use crate::files;
use crate::repo::files::title_from_path;

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

    let expanded = expand_directives(md, &mut ctx).await;

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&expanded, opts);
    let events = highlight_code_block_events(parser.collect());
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());

    rewrite_internal_links(&out)
}

// ---------------------------------------------------------------------------
// Server-side syntax highlighting of fenced code blocks
// ---------------------------------------------------------------------------

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static HIGHLIGHT_THEME: LazyLock<Theme> = LazyLock::new(|| {
    let mut themes = ThemeSet::load_defaults();
    themes
        .themes
        .remove("InspiredGitHub")
        .expect("InspiredGitHub theme is bundled with syntect defaults")
});

/// Rewrite the event stream so each fenced code block becomes a single
/// `Event::Html` carrying server-highlighted markup. All other events pass
/// through untouched.
fn highlight_code_block_events(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out: Vec<Event<'_>> = Vec::with_capacity(events.len());
    let mut iter = events.into_iter();
    while let Some(ev) = iter.next() {
        if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang))) = &ev {
            let lang = lang.clone().into_string();
            let mut code = String::new();
            for inner in iter.by_ref() {
                match inner {
                    Event::End(TagEnd::CodeBlock) => break,
                    Event::Text(t) | Event::Code(t) | Event::Html(t) => code.push_str(&t),
                    _ => {}
                }
            }
            out.push(Event::Html(highlight_code_block(&lang, &code).into()));
        } else {
            out.push(ev);
        }
    }
    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Produce the HTML for one fenced code block. Chess directives (`fen`/`pgn`)
/// keep their `language-<lang>` class so the client chess viewer still picks
/// them up; everything else is highlighted with syntect (plain-text fallback
/// when the language is empty or unknown).
///
/// Contract: every non-chess block is a single `<pre class="code-block" ...>`
/// element with an optional `data-lang` attribute and no blank lines inside, so
/// a frontend enhancer can find and decorate it reliably.
fn highlight_code_block(lang: &str, code: &str) -> String {
    let lang = lang.trim().to_ascii_lowercase();

    if lang == "fen" || lang == "pgn" {
        return format!(
            "<pre><code class=\"language-{lang}\">{}</code></pre>",
            escape_html(code)
        );
    }

    let data_lang = if lang.is_empty() {
        String::new()
    } else {
        format!(" data-lang=\"{}\"", escape_html(&lang))
    };

    let syntax = if lang.is_empty() {
        None
    } else {
        SYNTAX_SET.find_syntax_by_token(&lang)
    };

    let Some(syntax) = syntax else {
        // Plain fallback: no syntect, just escape and wrap.
        return format!(
            "<pre class=\"code-block\"{data_lang}><code>{}</code></pre>",
            escape_html(code)
        );
    };

    match highlighted_html_for_string(code, &SYNTAX_SET, syntax, &HIGHLIGHT_THEME) {
        Ok(html) => {
            let inner = strip_outer_pre(&html);
            format!("<pre class=\"code-block\"{data_lang}>{inner}</pre>")
        }
        Err(e) => {
            tracing::warn!(error = %e, lang = %lang, "syntect highlight failed; falling back to plain");
            format!(
                "<pre class=\"code-block\"{data_lang}><code>{}</code></pre>",
                escape_html(code)
            )
        }
    }
}

/// Strip syntect's outer `<pre ...>` opening tag and trailing `</pre>`,
/// returning the inner highlighted spans.
fn strip_outer_pre(html: &str) -> &str {
    let inner = match html.find('>') {
        Some(idx) if html.starts_with("<pre") => &html[idx + 1..],
        _ => html,
    };
    inner.strip_suffix("</pre>").unwrap_or(inner)
}

// ---------------------------------------------------------------------------
// Directive parsing
// ---------------------------------------------------------------------------

struct Directive {
    /// Internal handler name (`page`/`file`/`img`/`gallery`/`fen`/`pgn`/`mermaid`).
    name: String,
    args: HashMap<String, String>,
    /// Body of a paired container tag (`<fen>…</fen>`, `<pgn>…</pgn>`); `None`
    /// for void directives.
    inner: Option<String>,
}

impl Directive {
    fn arg(&self, key: &str) -> Option<&str> {
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
fn parse_tag_directive(s: &str) -> Option<(Directive, usize)> {
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

// ---------------------------------------------------------------------------
// Expansion: walk lines, skip code, dispatch directives
// ---------------------------------------------------------------------------

fn expand_directives<'a>(
    md: &'a str,
    ctx: &'a mut RenderCtx<'_>,
) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
    Box::pin(expand_directives_impl(md, ctx))
}

async fn expand_directives_impl(md: &str, ctx: &mut RenderCtx<'_>) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut fence_char = '`';

    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let raw_line = lines[i];
        let stripped = raw_line.trim_end_matches(['\n', '\r']);
        let trimmed = stripped.trim_start();

        if in_fence {
            out.push_str(raw_line);
            let close = if fence_char == '`' { "```" } else { "~~~" };
            if trimmed.starts_with(close) {
                in_fence = false;
            }
            i += 1;
            continue;
        }
        // A fenced code block tagged `mermaid` (` ```mermaid ` / `~~~mermaid`) is
        // rendered as a diagram rather than passed through to the highlighter.
        // Any other fence (including an untagged one showing directive syntax as
        // an example) is left alone below.
        if (trimmed.starts_with("```") || trimmed.starts_with("~~~"))
            && let Some((dir, consumed_lines)) = collect_mermaid_fence(&lines, i)
        {
            let html = dispatch_directive(&dir, ctx).await;
            out.push_str(&html);
            i += consumed_lines;
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = true;
            fence_char = '`';
            out.push_str(raw_line);
            i += 1;
            continue;
        }
        if trimmed.starts_with("~~~") {
            in_fence = true;
            fence_char = '~';
            out.push_str(raw_line);
            i += 1;
            continue;
        }

        // Paired container directive (`<fen>…</fen>` / `<pgn>…</pgn>`), which may
        // span several lines. Handled here because the inline scanner is per-line.
        if let Some((dir, consumed_lines)) = collect_container(&lines, i) {
            let html = dispatch_directive(&dir, ctx).await;
            out.push_str(&html);
            i += consumed_lines;
            continue;
        }

        let trailing = &raw_line[stripped.len()..];
        let expanded = expand_line_directives(stripped, ctx).await;
        out.push_str(&expanded);
        out.push_str(trailing);
        i += 1;
    }

    out
}

/// If `lines[start]` opens a paired `<fen>`/`<pgn>`/`<mermaid>` container, return
/// the directive (with `inner` set to the body) and the number of lines consumed.
/// A file-backed `<fen path=...>` on its own line is *not* a container; nor is a
/// line that doesn't start (after trimming) with one of these tags.
fn collect_container(lines: &[&str], start: usize) -> Option<(Directive, usize)> {
    let open_line = lines[start].trim_end_matches(['\n', '\r']);
    let trimmed = open_line.trim_start();
    if !(trimmed.starts_with("<fen")
        || trimmed.starts_with("<pgn")
        || trimmed.starts_with("<mermaid")
        || trimmed.starts_with("<json"))
    {
        return None;
    }
    let (mut dir, consumed) = parse_tag_directive(trimmed)?;
    if dir.name != "fen" && dir.name != "pgn" && dir.name != "mermaid" && dir.name != "json" {
        return None;
    }
    let close = format!("</{}>", dir.name);
    let after = &trimmed[consumed..];

    // Same-line container: `<fen ...>BODY</fen>`.
    if let Some(idx) = after.find(close.as_str()) {
        if !after[idx + close.len()..].trim().is_empty() {
            return None; // trailing content → not a clean block container
        }
        dir.inner = Some(after[..idx].to_string());
        return Some((dir, 1));
    }

    // Multi-line container: the opening tag must stand alone on its line, and the
    // tag must carry no file-lookup attr (which would mark the void form).
    if !after.trim().is_empty() {
        return None;
    }
    if dir.arg("path").is_some() || dir.arg("id").is_some() || dir.arg("hash").is_some() {
        return None;
    }
    let mut body = String::new();
    let mut j = start + 1;
    while j < lines.len() {
        let line = lines[j];
        let line_trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(idx) = line_trimmed.find(close.as_str()) {
            if !line_trimmed[idx + close.len()..].trim().is_empty() {
                return None;
            }
            body.push_str(&line_trimmed[..idx]);
            dir.inner = Some(body);
            return Some((dir, j - start + 1));
        }
        body.push_str(line); // keep the newline
        j += 1;
    }
    None // unterminated
}

/// If `lines[start]` opens a fenced code block whose info string is `mermaid`
/// (` ```mermaid ` / `~~~mermaid`), collect its body as an inline `<mermaid>`
/// directive and return it with the number of lines consumed (through the
/// closing fence). Returns `None` for any other fence — untagged (e.g. a
/// fence used in documentation to show the `<mermaid>` tag literally), a
/// different language, or missing a closing line — so the caller falls back
/// to passing it through to the normal code-block highlighter unchanged.
fn collect_mermaid_fence(lines: &[&str], start: usize) -> Option<(Directive, usize)> {
    let open_line = lines[start].trim_end_matches(['\n', '\r']);
    let trimmed = open_line.trim_start();
    let (marker, close) = if trimmed.starts_with("```") {
        ("```", "```")
    } else {
        ("~~~", "~~~")
    };
    let info = trimmed[marker.len()..].trim();
    let lang = info.split_whitespace().next().unwrap_or("");
    if !lang.eq_ignore_ascii_case("mermaid") {
        return None;
    }

    let mut body = String::new();
    let mut j = start + 1;
    while j < lines.len() {
        let line = lines[j];
        let line_trimmed = line.trim_end_matches(['\n', '\r']).trim_start();
        if line_trimmed.starts_with(close) {
            let dir = Directive {
                name: "mermaid".to_string(),
                args: HashMap::new(),
                inner: Some(body),
            };
            return Some((dir, j - start + 1));
        }
        body.push_str(line);
        j += 1;
    }
    None // unterminated fence — let the normal fence handling pass it through
}

/// Walk a line and expand any `<name ...>` directive outside inline code spans.
/// Backtick spans, and any `<tag>` not on the allow-list, are passed through.
async fn expand_line_directives(line: &str, ctx: &mut RenderCtx<'_>) -> String {
    let mut out = String::new();
    let mut rest = line;

    while !rest.is_empty() {
        if rest.starts_with('`') {
            let tick_count = rest.bytes().take_while(|&b| b == b'`').count();
            let ticks = &rest[..tick_count];
            if let Some(close) = rest[tick_count..].find(ticks) {
                let end = tick_count + close + tick_count;
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            } else {
                out.push_str(rest);
                rest = "";
            }
        } else if rest.starts_with('<') {
            if let Some((d, consumed)) = parse_tag_directive(rest) {
                let expansion = dispatch_directive(&d, ctx).await;
                out.push_str(&expansion);
                rest = &rest[consumed..];
            } else {
                out.push('<');
                rest = &rest[1..];
            }
        } else {
            let next = rest.find(['`', '<']).unwrap_or(rest.len());
            if next == 0 {
                let ch = rest.chars().next().unwrap();
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            } else {
                out.push_str(&rest[..next]);
                rest = &rest[next..];
            }
        }
    }

    out
}

async fn dispatch_directive(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    match d.name.as_str() {
        "page" => directive_page(d, ctx).await,
        "file" => directive_file(d, ctx).await,
        "img" => directive_img(d, ctx).await,
        "gallery" => directive_gallery(d, ctx).await,
        "fen" => directive_fen(d, ctx).await,
        "pgn" => directive_pgn(d, ctx).await,
        "mermaid" => directive_mermaid(d, ctx).await,
        "json" => directive_json(d, ctx).await,
        unknown => format!("\n\n*[unknown directive `<{unknown}>`]*\n\n"),
    }
}

/// Wrap rendered directive HTML so it lands as its own raw-HTML block when the
/// markdown parser runs. The surrounding blank lines keep markdown from
/// re-parsing the HTML.
///
/// Constraint: the rendered HTML must contain no whitespace-only lines, since
/// CommonMark closes a raw-HTML block at the first blank line — any content
/// after would be re-parsed and `<` chars would be escaped. Template authors
/// must use whitespace-stripping markers (`{%- ... %}`) around control tags
/// inside loops.
fn block(html: String) -> String {
    format!("\n\n{html}\n\n")
}

/// Render a `markdown/<name>.html` template; on failure, log and emit a
/// visible inline error so authors can spot it.
fn render_md_template(ctx: &RenderCtx<'_>, name: &str, tctx: minijinja::value::Value) -> String {
    let path = format!("markdown/{name}.html");
    match ctx.tmpl.get_template(&path) {
        Ok(t) => match t.render(tctx) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(template = %path, error = %e, "markdown template render failed");
                format!("<p><em>[template `{path}` render failed]</em></p>")
            }
        },
        Err(e) => {
            tracing::error!(template = %path, error = %e, "markdown template missing");
            format!("<p><em>[template `{path}` missing]</em></p>")
        }
    }
}

// ---------------------------------------------------------------------------
// File / gallery lookup
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum FileLookup {
    Path(String),
    Id(i32),
    Hash(String),
}

fn parse_file_lookup(d: &Directive, name: &str) -> Result<FileLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());
    let hash = d.arg("hash").filter(|s| !s.is_empty());

    let count = path.is_some() as u8 + id.is_some() as u8 + hash.is_some() as u8;
    match count {
        0 => Err(format!(
            "\n\n*[`<{name}>` requires `path`, `id`, or `hash`]*\n\n"
        )),
        1 => {
            if let Some(p) = path {
                Ok(FileLookup::Path(p.to_owned()))
            } else if let Some(i) = id {
                let n: i32 = i.parse().map_err(|_| {
                    format!("\n\n*[`<{name}>` got invalid `id` (expected integer)]*\n\n")
                })?;
                Ok(FileLookup::Id(n))
            } else {
                let h = hash.unwrap().to_ascii_lowercase();
                if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(format!(
                        "\n\n*[`<{name}>` got invalid `hash` (expected 64 hex chars)]*\n\n"
                    ));
                }
                Ok(FileLookup::Hash(h))
            }
        }
        _ => Err(format!(
            "\n\n*[`<{name}>` accepts only one of `path`, `id`, `hash`]*\n\n"
        )),
    }
}

async fn fetch_file(db: &DatabaseConnection, lookup: &FileLookup) -> Option<file_entity::Model> {
    match lookup {
        FileLookup::Id(id) => file_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        FileLookup::Hash(h) => file_entity::Entity::find()
            .filter(file_entity::Column::Hash.eq(h.as_str()))
            .one(db)
            .await
            .ok()
            .flatten(),
        FileLookup::Path(p) => file_entity::Entity::find()
            .filter(file_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}

fn lookup_label(lookup: &FileLookup) -> String {
    match lookup {
        FileLookup::Path(p) => p.clone(),
        FileLookup::Id(i) => i.to_string(),
        FileLookup::Hash(h) => h.clone(),
    }
}

#[derive(Debug)]
enum GalleryLookup {
    Path(String),
    Id(i32),
}

fn parse_gallery_lookup(d: &Directive) -> Result<GalleryLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());

    match (path, id) {
        (Some(p), None) => Ok(GalleryLookup::Path(p.to_owned())),
        (None, Some(i)) => {
            let n: i32 = i.parse().map_err(|_| {
                "\n\n*[`<gallery>` got invalid `id` (expected integer)]*\n\n".to_owned()
            })?;
            Ok(GalleryLookup::Id(n))
        }
        (Some(_), Some(_)) => {
            Err("\n\n*[`<gallery>` accepts only one of `path`, `id`]*\n\n".to_owned())
        }
        (None, None) => Err("\n\n*[`<gallery>` requires `path` or `id`]*\n\n".to_owned()),
    }
}

#[derive(Debug)]
enum PageLookup {
    Path(String),
    Id(i32),
}

fn parse_page_lookup(d: &Directive) -> Result<PageLookup, String> {
    let path = d.arg("path").filter(|s| !s.is_empty());
    let id = d.arg("id").filter(|s| !s.is_empty());

    match (path, id) {
        (Some(p), None) => Ok(PageLookup::Path(p.to_owned())),
        (None, Some(i)) => {
            let n: i32 = i.parse().map_err(|_| {
                "\n\n*[`<page>` got invalid `id` (expected integer)]*\n\n".to_owned()
            })?;
            Ok(PageLookup::Id(n))
        }
        (Some(_), Some(_)) => {
            Err("\n\n*[`<page>` accepts only one of `path`, `id`]*\n\n".to_owned())
        }
        (None, None) => Err("\n\n*[`<page>` requires `path` or `id`]*\n\n".to_owned()),
    }
}

async fn fetch_page(db: &DatabaseConnection, lookup: &PageLookup) -> Option<page_entity::Model> {
    match lookup {
        PageLookup::Id(id) => page_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        PageLookup::Path(p) => page_entity::Entity::find()
            .filter(page_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}

async fn fetch_gallery(
    db: &DatabaseConnection,
    lookup: &GalleryLookup,
) -> Option<gallery_entity::Model> {
    match lookup {
        GalleryLookup::Id(id) => gallery_entity::Entity::find_by_id(*id)
            .one(db)
            .await
            .ok()
            .flatten(),
        GalleryLookup::Path(p) => gallery_entity::Entity::find()
            .filter(gallery_entity::Column::Path.eq(crate::path_util::normalize(p)))
            .one(db)
            .await
            .ok()
            .flatten(),
    }
}

// ---------------------------------------------------------------------------
// <page path|id=...>
// ---------------------------------------------------------------------------

async fn directive_page(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_page_lookup(d) {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(page) = fetch_page(ctx.db, &lookup).await else {
        let label = match &lookup {
            PageLookup::Id(i) => i.to_string(),
            PageLookup::Path(p) => p.clone(),
        };
        let html = format!(r#"<p><em>[page "{label}" not found]</em></p>"#);
        return block(html);
    };

    if page.private && !ctx.logged_in {
        return String::new();
    }

    let path = page.path.clone();
    if ctx.visited_pages.contains(&path) {
        let html = format!(r#"<p><em>[recursive transclusion of "{path}" skipped]</em></p>"#);
        return block(html);
    }

    ctx.visited_pages.insert(path.clone());
    let nested = expand_directives(&page.markdown, ctx).await;
    ctx.visited_pages.remove(&path);

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&nested, opts);
    let mut inner_html = String::new();
    html::push_html(&mut inner_html, parser);

    let html = render_md_template(
        ctx,
        "page",
        context! { path => &path, inner_html => &inner_html },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <file path|id|hash=...>  — image if mime image/*, else download link
// ---------------------------------------------------------------------------

async fn directive_file(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_file_lookup(d, "file") {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(file) = fetch_file(ctx.db, &lookup).await else {
        let label = lookup_label(&lookup);
        let html = format!(r#"<p><em>[file "{label}" not found]</em></p>"#);
        return block(html);
    };

    let title = title_from_path(&file.path);
    if file.mimetype.starts_with("image/") {
        let html = render_md_template(
            ctx,
            "img",
            context! { hash => &file.hash, title => &title, alt => &title },
        );
        return block(html);
    }

    let description = file
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(title.as_str());
    let html = render_md_template(
        ctx,
        "file",
        context! { hash => &file.hash, title => &title, description },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <image path|id|hash=... alt=...>  — force image embed
// ---------------------------------------------------------------------------

async fn directive_img(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_file_lookup(d, "image") {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(file) = fetch_file(ctx.db, &lookup).await else {
        let label = lookup_label(&lookup);
        let html = format!(r#"<p><em>[image "{label}" not found]</em></p>"#);
        return block(html);
    };

    let title = title_from_path(&file.path);
    let alt = d
        .arg("alt")
        .filter(|s| !s.is_empty())
        .unwrap_or(title.as_str());
    let html = render_md_template(
        ctx,
        "img",
        context! { hash => &file.hash, title => &title, alt },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <gallery path|id=...>
// ---------------------------------------------------------------------------

async fn directive_gallery(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let lookup = match parse_gallery_lookup(d) {
        Ok(l) => l,
        Err(msg) => return msg,
    };

    let Some(gal) = fetch_gallery(ctx.db, &lookup).await else {
        let label = match &lookup {
            GalleryLookup::Id(i) => i.to_string(),
            GalleryLookup::Path(p) => p.clone(),
        };
        let html = format!(r#"<p><em>[gallery "{label}" not found]</em></p>"#);
        return block(html);
    };

    #[derive(serde::Serialize)]
    struct GalleryItem {
        hash: String,
        title: String,
    }

    let mut items: Vec<GalleryItem> = Vec::with_capacity(gal.file_ids.len());
    for file_id in &gal.file_ids {
        if let Ok(Some(img)) = file_entity::Entity::find_by_id(*file_id).one(ctx.db).await {
            items.push(GalleryItem {
                hash: img.hash,
                title: title_from_path(&img.path),
            });
        }
    }

    let html = render_md_template(
        ctx,
        "gallery",
        context! { id => gal.id, title => &gal.title, items => &items },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <fen path|id|hash=... size=small|large>  — file-backed, or
// <fen size=...>FEN string</fen>            — inline body
// ---------------------------------------------------------------------------

async fn directive_fen(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let size_class = parse_size_class(d);

    let fen = match inline_body(d) {
        Some(body) => body,
        None => {
            let lookup = match parse_file_lookup(d, "fen") {
                Ok(l) => l,
                Err(msg) => return msg,
            };
            let Some(file) = fetch_file(ctx.db, &lookup).await else {
                let label = lookup_label(&lookup);
                let html = format!(r#"<p><em>[fen file "{label}" not found]</em></p>"#);
                return block(html);
            };
            let Some(fen) = read_text_blob(ctx.db, &file.hash).await else {
                let html = format!(r#"<p><em>[fen blob for "{}" missing]</em></p>"#, file.path);
                return block(html);
            };
            fen
        }
    };

    let html = render_md_template(
        ctx,
        "fen",
        context! { fen => fen.trim(), size_class => size_class },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <pgn path|id|hash=... size=small|large move=N>  — file-backed, or
// <pgn size=... move=N>PGN moves</pgn>             — inline body
// ---------------------------------------------------------------------------

async fn directive_pgn(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let size_class = parse_size_class(d);
    let move_attr = d.arg("move").filter(|s| !s.is_empty());

    let pgn = match inline_body(d) {
        Some(body) => body,
        None => {
            let lookup = match parse_file_lookup(d, "pgn") {
                Ok(l) => l,
                Err(msg) => return msg,
            };
            let Some(file) = fetch_file(ctx.db, &lookup).await else {
                let label = lookup_label(&lookup);
                let html = format!(r#"<p><em>[pgn file "{label}" not found]</em></p>"#);
                return block(html);
            };
            let Some(pgn) = read_text_blob(ctx.db, &file.hash).await else {
                let html = format!(r#"<p><em>[pgn blob for "{}" missing]</em></p>"#, file.path);
                return block(html);
            };
            pgn
        }
    };

    let html = render_md_template(
        ctx,
        "pgn",
        context! {
            pgn => pgn.trim(),
            size_class => size_class,
            move => move_attr,
        },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <mermaid path|id|hash=... theme=default|dark|forest|neutral>  — file-backed, or
// <mermaid theme=...>DIAGRAM</mermaid>                          — inline body.
// Rendered to SVG server-side; on failure the source is shown in a code block.
// ---------------------------------------------------------------------------

async fn directive_mermaid(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let size_class = parse_size_class(d);
    let theme = mermaid_svg::Theme::by_name(&d.arg("theme").unwrap_or("").to_ascii_lowercase())
        .unwrap_or_default();

    let source = match inline_body(d) {
        Some(body) => body,
        None => {
            let lookup = match parse_file_lookup(d, "mermaid") {
                Ok(l) => l,
                Err(msg) => return msg,
            };
            let Some(file) = fetch_file(ctx.db, &lookup).await else {
                let label = lookup_label(&lookup);
                let html = format!(r#"<p><em>[mermaid file "{label}" not found]</em></p>"#);
                return block(html);
            };
            let Some(src) = read_text_blob(ctx.db, &file.hash).await else {
                let html = format!(
                    r#"<p><em>[mermaid blob for "{}" missing]</em></p>"#,
                    file.path
                );
                return block(html);
            };
            src
        }
    };
    let source = source.trim().to_string();

    // mermaid-svg rendering is synchronous and CPU-bound (graph layout); keep it
    // off the async worker. On any failure, leave `svg` empty so the template
    // falls back to showing the raw source in a code block.
    let src = source.clone();
    let svg =
        match tokio::task::spawn_blocking(move || mermaid_svg::render_with(&src, &theme)).await {
            Ok(Ok(svg)) => svg,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "mermaid render failed; showing source");
                String::new()
            }
            Err(e) => {
                tracing::error!(error = %e, "mermaid render task panicked");
                String::new()
            }
        };

    let html = render_md_template(
        ctx,
        "mermaid",
        context! { svg => svg, source => source, size_class => size_class },
    );
    block(html)
}

// ---------------------------------------------------------------------------
// <json path|id|hash=... query=".rows[]" type="table">  — file-backed, or
// <json query="..." type="...">{ ...json... }</json>     — inline body.
// Runs a jq query (jaq) over the JSON and renders the result (default: table).
// ---------------------------------------------------------------------------

async fn directive_json(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
    let query = match d.arg("query").filter(|s| !s.is_empty()) {
        Some(q) => q,
        None => return "\n\n*[json: missing `query`]*\n\n".to_string(),
    };
    let kind = d.arg("type").filter(|s| !s.is_empty()).unwrap_or("table");
    if kind != "table" {
        return format!("\n\n*[json: unknown type \"{kind}\"]*\n\n");
    }

    let source = match inline_body(d) {
        Some(body) => body,
        None => {
            let lookup = match parse_file_lookup(d, "json") {
                Ok(l) => l,
                Err(msg) => return msg,
            };
            let Some(file) = fetch_file(ctx.db, &lookup).await else {
                return format!(
                    "\n\n*[json: file \"{}\" not found]*\n\n",
                    lookup_label(&lookup)
                );
            };
            let Some(src) = read_text_blob(ctx.db, &file.hash).await else {
                return format!("\n\n*[json: blob for \"{}\" missing]*\n\n", file.path);
            };
            src
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&source) {
        Ok(v) => v,
        Err(e) => return format!("\n\n*[json: invalid JSON: {e}]*\n\n"),
    };

    let outputs = match run_jq(query, value) {
        Ok(o) => o,
        Err(e) => return format!("\n\n*[json: jq error: {e}]*\n\n"),
    };

    let (columns, rows) = match json_table(outputs) {
        Ok(t) => t,
        Err(e) => return format!("\n\n*[json: {e}]*\n\n"),
    };

    let html = render_md_template(
        ctx,
        "json",
        context! { kind => kind, columns => columns, rows => rows },
    );
    block(html)
}

/// Run a jq query over `input` using jaq, collecting all outputs.
fn run_jq(query: &str, input: serde_json::Value) -> Result<Vec<serde_json::Value>, String> {
    use jaq_core::load::{Arena, File, Loader};
    use jaq_core::{Compiler, Ctx, RcIter};
    use jaq_json::Val;

    let arena = Arena::default();
    let loader = Loader::new(jaq_std::defs().chain(jaq_json::defs()));
    let modules = loader
        .load(
            &arena,
            File {
                code: query,
                path: (),
            },
        )
        .map_err(|errs| format!("{errs:?}"))?;
    let filter = Compiler::default()
        .with_funs(jaq_std::funs().chain(jaq_json::funs()))
        .compile(modules)
        .map_err(|errs| format!("{errs:?}"))?;

    let inputs = RcIter::new(core::iter::empty());
    let ctx = Ctx::new([], &inputs);
    let mut out = Vec::new();
    for r in filter.run((ctx, Val::from(input))) {
        out.push(serde_json::Value::from(r.map_err(|e| e.to_string())?));
    }
    Ok(out)
}

/// Flatten jq outputs into table columns + rows.
///
/// Each top-level output that is itself an array is expanded into its items, so
/// `.rows[]` and `.rows` both work. Object items contribute a header row (the
/// first-seen union of keys); array items become header-less cell rows. Mixing
/// objects and arrays is rejected.
fn json_table(outputs: Vec<serde_json::Value>) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    use serde_json::Value;

    // A single jq output that is an array of objects (e.g. `.rows`) is the table
    // itself, so expand it into rows. An array of non-objects (e.g. `[1,2,3]`)
    // stays one item: it's a single cell-row.
    let mut items: Vec<Value> = Vec::new();
    for out in outputs {
        match out {
            Value::Array(arr) if arr.iter().all(Value::is_object) && !arr.is_empty() => {
                items.extend(arr)
            }
            other => items.push(other),
        }
    }

    if items.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let all_objects = items.iter().all(|v| v.is_object());
    let all_arrays = items.iter().all(|v| v.is_array());

    if all_objects {
        let mut columns: Vec<String> = Vec::new();
        for item in &items {
            for key in item.as_object().unwrap().keys() {
                if !columns.iter().any(|c| c == key) {
                    columns.push(key.clone());
                }
            }
        }
        let rows = items
            .iter()
            .map(|item| {
                let obj = item.as_object().unwrap();
                columns
                    .iter()
                    .map(|c| obj.get(c).map(stringify_cell).unwrap_or_default())
                    .collect()
            })
            .collect();
        Ok((columns, rows))
    } else if all_arrays {
        let rows = items
            .iter()
            .map(|item| {
                item.as_array()
                    .unwrap()
                    .iter()
                    .map(stringify_cell)
                    .collect()
            })
            .collect();
        Ok((Vec::new(), rows))
    } else {
        Err("non-tabular result (expected objects or arrays)".to_string())
    }
}

/// Stringify a scalar cell: strings bare, numbers/bools as-is, null → empty,
/// nested objects/arrays → compact JSON.
fn stringify_cell(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Trimmed paired-tag body, if the directive carries a non-empty one.
fn inline_body(d: &Directive) -> Option<String> {
    d.inner
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_size_class(d: &Directive) -> &'static str {
    match d.arg("size").unwrap_or("") {
        "small" | "sm" => " size-sm",
        "large" | "lg" => " size-lg",
        _ => "",
    }
}

async fn read_text_blob(db: &DatabaseConnection, hash: &str) -> Option<String> {
    let bytes = files::read_blob(db, hash).await.ok().flatten()?;
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// Internal links: rewrite relative href values to page paths
// [Syncthing](Infra/Desktop/Syncthing.md) → <a href="/infra/desktop/syncthing">
// ---------------------------------------------------------------------------

fn rewrite_internal_links(html: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_block(line: &str) -> Option<Directive> {
        let trimmed = line.trim();
        let (d, n) = parse_tag_directive(trimmed)?;
        if n == trimmed.len() { Some(d) } else { None }
    }

    #[test]
    fn parse_page_path() {
        let d = parse_block(r#"<page path="infra/desktop/syncthing">"#).unwrap();
        assert_eq!(d.name, "page");
        assert_eq!(d.arg("path"), Some("infra/desktop/syncthing"));
    }

    #[test]
    fn parse_unquoted_path_keeps_slashes() {
        let d = parse_block("<page path=infra/desktop/syncthing>").unwrap();
        assert_eq!(d.arg("path"), Some("infra/desktop/syncthing"));
    }

    #[test]
    fn parse_quoted_value_with_spaces() {
        let d = parse_block(r#"<page path="my page/with spaces">"#).unwrap();
        assert_eq!(d.arg("path"), Some("my page/with spaces"));
    }

    #[test]
    fn parse_multi_args() {
        let d = parse_block(r#"<pgn hash="ab12" size="large" move="12">"#).unwrap();
        assert_eq!(d.arg("hash"), Some("ab12"));
        assert_eq!(d.arg("size"), Some("large"));
        assert_eq!(d.arg("move"), Some("12"));
    }

    #[test]
    fn image_canonicalizes_to_img() {
        let d = parse_block(r#"<image hash="abc">"#).unwrap();
        assert_eq!(d.name, "img");
        assert_eq!(d.arg("hash"), Some("abc"));
    }

    #[test]
    fn allowlist_rejects_real_html() {
        assert!(parse_tag_directive(r#"<div class="x">"#).is_none());
        assert!(parse_tag_directive(r#"<img src="x.png">"#).is_none());
        assert!(parse_tag_directive("</page>").is_none());
    }

    #[test]
    fn rejects_unterminated_tag() {
        assert!(parse_tag_directive("<page path=x").is_none());
    }

    #[test]
    fn quoted_value_keeps_gt() {
        let (d, _) = parse_tag_directive(r#"<pgn move="1. e4 > 0">"#).unwrap();
        assert_eq!(d.arg("move"), Some("1. e4 > 0"));
    }

    #[test]
    fn rejects_old_colon_directive() {
        assert!(parse_tag_directive("::page{path=foo}").is_none());
        assert!(parse_block("![[some/page]]").is_none());
    }

    #[test]
    fn tag_with_trailing_text_consumes_only_tag() {
        let (d, n) = parse_tag_directive(r#"<image hash="abc"> hello"#).unwrap();
        assert_eq!(d.name, "img");
        assert_eq!(n, r#"<image hash="abc">"#.len());
    }

    #[test]
    fn container_same_line() {
        let lines: Vec<&str> = "<fen>rnbq w - 0 1</fen>\n".split_inclusive('\n').collect();
        let (d, consumed) = collect_container(&lines, 0).unwrap();
        assert_eq!(d.name, "fen");
        assert_eq!(d.inner.as_deref(), Some("rnbq w - 0 1"));
        assert_eq!(consumed, 1);
    }

    #[test]
    fn container_multi_line() {
        let md = "<pgn size=\"large\">\n1. e4 e5\n2. Nf3 Nc6\n</pgn>\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        let (d, consumed) = collect_container(&lines, 0).unwrap();
        assert_eq!(d.name, "pgn");
        assert_eq!(d.arg("size"), Some("large"));
        assert_eq!(d.inner.as_deref(), Some("1. e4 e5\n2. Nf3 Nc6\n"));
        assert_eq!(consumed, 4);
    }

    #[test]
    fn container_void_fen_not_collected() {
        let lines: Vec<&str> = "<fen path=\"x.fen\">\n".split_inclusive('\n').collect();
        assert!(collect_container(&lines, 0).is_none());
    }

    #[test]
    fn container_mermaid_multi_line() {
        let md = "<mermaid theme=\"dark\">\ngraph TD\n  A --> B\n</mermaid>\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        let (d, consumed) = collect_container(&lines, 0).unwrap();
        assert_eq!(d.name, "mermaid");
        assert_eq!(d.arg("theme"), Some("dark"));
        assert_eq!(d.inner.as_deref(), Some("graph TD\n  A --> B\n"));
        assert_eq!(consumed, 4);
    }

    #[test]
    fn mermaid_renders_svg() {
        let svg = mermaid_svg::render("pie\n\"A\" : 1\n\"B\" : 2\n").unwrap();
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn fence_mermaid_collected() {
        let md = "```mermaid\ngraph TD\n  A --> B\n```\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        let (d, consumed) = collect_mermaid_fence(&lines, 0).unwrap();
        assert_eq!(d.name, "mermaid");
        assert_eq!(d.inner.as_deref(), Some("graph TD\n  A --> B\n"));
        assert_eq!(consumed, 4);
    }

    #[test]
    fn fence_tilde_mermaid_collected() {
        let md = "~~~mermaid\npie\n\"A\" : 1\n~~~\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        let (d, consumed) = collect_mermaid_fence(&lines, 0).unwrap();
        assert_eq!(d.name, "mermaid");
        assert_eq!(d.inner.as_deref(), Some("pie\n\"A\" : 1\n"));
        assert_eq!(consumed, 4);
    }

    #[test]
    fn fence_other_lang_not_collected() {
        let md = "```rust\nfn main() {}\n```\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        assert!(collect_mermaid_fence(&lines, 0).is_none());
    }

    #[test]
    fn fence_untagged_not_collected() {
        let md = "```\n<mermaid>\ngraph TD\n  A --> B\n</mermaid>\n```\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        assert!(collect_mermaid_fence(&lines, 0).is_none());
    }

    #[test]
    fn fence_unterminated_mermaid_not_collected() {
        let md = "```mermaid\ngraph TD\n  A --> B\n";
        let lines: Vec<&str> = md.split_inclusive('\n').collect();
        assert!(collect_mermaid_fence(&lines, 0).is_none());
    }

    fn make_dir(name: &str, args: &[(&str, &str)]) -> Directive {
        let mut map = HashMap::new();
        for (k, v) in args {
            map.insert((*k).to_owned(), (*v).to_owned());
        }
        Directive {
            name: name.to_owned(),
            args: map,
            inner: None,
        }
    }

    #[test]
    fn file_lookup_id() {
        let d = make_dir("file", &[("id", "42")]);
        match parse_file_lookup(&d, "file").unwrap() {
            FileLookup::Id(n) => assert_eq!(n, 42),
            other => panic!("expected id, got {other:?}"),
        }
    }

    #[test]
    fn file_lookup_hash_lowercases() {
        let h = "A".repeat(64);
        let d = make_dir("img", &[("hash", &h)]);
        match parse_file_lookup(&d, "img").unwrap() {
            FileLookup::Hash(out) => assert_eq!(out, "a".repeat(64)),
            other => panic!("expected hash, got {other:?}"),
        }
    }

    #[test]
    fn file_lookup_rejects_invalid_hash() {
        let d = make_dir("img", &[("hash", "deadbeef")]);
        let err = parse_file_lookup(&d, "img").unwrap_err();
        assert!(err.contains("invalid `hash`"));
    }

    #[test]
    fn file_lookup_rejects_two_keys() {
        let d = make_dir("file", &[("path", "a"), ("id", "1")]);
        let err = parse_file_lookup(&d, "file").unwrap_err();
        assert!(err.contains("only one"));
    }

    #[test]
    fn file_lookup_rejects_empty() {
        let d = make_dir("file", &[]);
        let err = parse_file_lookup(&d, "file").unwrap_err();
        assert!(err.contains("requires"));
    }

    #[test]
    fn highlight_rust_block() {
        let html = highlight_code_block("rust", "fn main() {}\n");
        assert!(html.starts_with("<pre class=\"code-block\""), "got: {html}");
        assert!(html.contains("data-lang=\"rust\""), "got: {html}");
        assert!(html.ends_with("</pre>"));
        // syntect emits styled spans for highlighted code.
        assert!(html.contains("<span"), "expected highlighted spans: {html}");
    }

    #[test]
    fn highlight_fen_keeps_language_class() {
        let html = highlight_code_block("fen", "rnbq w - 0 1");
        assert_eq!(
            html,
            "<pre><code class=\"language-fen\">rnbq w - 0 1</code></pre>"
        );
    }

    #[test]
    fn highlight_pgn_keeps_language_class() {
        let html = highlight_code_block("pgn", "1. e4 e5");
        assert!(html.contains("class=\"language-pgn\""));
    }

    #[test]
    fn highlight_unknown_lang_plain_fallback() {
        let html = highlight_code_block("nosuchlang", "<a> & </a>");
        assert!(html.starts_with("<pre class=\"code-block\" data-lang=\"nosuchlang\">"));
        assert!(html.contains("&lt;a&gt; &amp; &lt;/a&gt;"));
        assert!(!html.contains("<span"));
    }

    #[test]
    fn highlight_empty_lang_no_data_attr() {
        let html = highlight_code_block("", "plain text");
        assert!(
            html.starts_with("<pre class=\"code-block\">"),
            "got: {html}"
        );
        assert!(!html.contains("data-lang"));
    }

    #[test]
    fn highlight_has_no_blank_lines() {
        let html = highlight_code_block("rust", "fn a() {}\n\nfn b() {}\n");
        assert!(
            !html.contains("\n\n"),
            "blank line in emitted HTML: {html:?}"
        );
    }

    fn jv(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn json_table_objects_union_keys() {
        let outputs = vec![jv(r#"{"a":1,"b":"x"}"#), jv(r#"{"b":"y","c":true}"#)];
        let (cols, rows) = json_table(outputs).unwrap();
        assert_eq!(cols, vec!["a", "b", "c"]);
        assert_eq!(rows[0], vec!["1", "x", ""]);
        assert_eq!(rows[1], vec!["", "y", "true"]);
    }

    #[test]
    fn json_table_expands_arrays_of_objects() {
        let outputs = vec![jv(r#"[{"a":1},{"a":2}]"#)];
        let (cols, rows) = json_table(outputs).unwrap();
        assert_eq!(cols, vec!["a"]);
        assert_eq!(rows, vec![vec!["1"], vec!["2"]]);
    }

    #[test]
    fn json_table_arrays_no_header() {
        let outputs = vec![jv("[1,2,3]"), jv(r#"["a","b"]"#)];
        let (cols, rows) = json_table(outputs).unwrap();
        assert!(cols.is_empty());
        assert_eq!(rows, vec![vec!["1", "2", "3"], vec!["a", "b"]]);
    }

    #[test]
    fn json_table_nested_cells_compact_json() {
        let outputs = vec![jv(r#"{"x":{"k":1},"y":null}"#)];
        let (_cols, rows) = json_table(outputs).unwrap();
        assert_eq!(rows[0], vec![r#"{"k":1}"#, ""]);
    }

    #[test]
    fn json_table_mixed_rejected() {
        let outputs = vec![jv(r#"{"a":1}"#), jv("[1,2]")];
        assert!(json_table(outputs).is_err());
    }

    #[test]
    fn run_jq_basic() {
        let out = run_jq(".rows[]", jv(r#"{"rows":[{"a":1},{"a":2}]}"#)).unwrap();
        assert_eq!(out, vec![jv(r#"{"a":1}"#), jv(r#"{"a":2}"#)]);
    }

    #[test]
    fn run_jq_invalid_query_errors() {
        assert!(run_jq("this is not valid jq @@", jv("{}")).is_err());
    }

    #[test]
    fn size_class_parsing() {
        let d = make_dir("pgn", &[("size", "large")]);
        assert_eq!(parse_size_class(&d), " size-lg");
        let d = make_dir("pgn", &[("size", "sm")]);
        assert_eq!(parse_size_class(&d), " size-sm");
        let d = make_dir("pgn", &[]);
        assert_eq!(parse_size_class(&d), "");
    }
}
