//! Expansion pipeline: walk the document line-by-line, skip fenced/inline
//! code, and dispatch recognized directives to their handlers.

use std::future::Future;
use std::pin::Pin;

use pulldown_cmark::{Options, Parser, html};

use super::RenderCtx;
use super::directives::{Directive, parse_tag_directive};
use super::handlers;

pub(super) fn expand_directives<'a>(
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
pub(super) fn collect_container(lines: &[&str], start: usize) -> Option<(Directive, usize)> {
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
pub(super) fn collect_mermaid_fence(lines: &[&str], start: usize) -> Option<(Directive, usize)> {
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
                args: std::collections::HashMap::new(),
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
        "page" => handlers::directive_page(d, ctx).await,
        "file" => handlers::directive_file(d, ctx).await,
        "img" => handlers::directive_img(d, ctx).await,
        "gallery" => handlers::directive_gallery(d, ctx).await,
        "fen" => handlers::directive_fen(d, ctx).await,
        "pgn" => handlers::directive_pgn(d, ctx).await,
        "mermaid" => handlers::directive_mermaid(d, ctx).await,
        "json" => handlers::directive_json(d, ctx).await,
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
pub(super) fn block(html: String) -> String {
    format!("\n\n{html}\n\n")
}

/// Render a `markdown/<name>.html` template; on failure, log and emit a
/// visible inline error so authors can spot it.
pub(super) fn render_md_template(
    ctx: &RenderCtx<'_>,
    name: &str,
    tctx: minijinja::value::Value,
) -> String {
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

/// Render nested page markdown (already directive-expanded) to inner HTML.
/// Shared by `render()` (top level) and the `<page>` handler (transclusion).
pub(super) fn render_expanded_to_html(expanded: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(expanded, opts);
    let mut inner_html = String::new();
    html::push_html(&mut inner_html, parser);
    inner_html
}
