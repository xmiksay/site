//! `<fen>`, `<pgn>`, `<mermaid>` directives — file-backed or inline-body forms.

use minijinja::context;

use super::super::RenderCtx;
use super::super::directives::Directive;
use super::super::lookup::{fetch_file, lookup_label, parse_file_lookup};
use super::super::renderer::{block, render_md_template};
use super::{inline_body, parse_size_class, read_text_blob};

// ---------------------------------------------------------------------------
// <fen path|id|hash=... size=small|large>  — file-backed, or
// <fen size=...>FEN string</fen>            — inline body
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_fen(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
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

pub(in crate::markdown) async fn directive_pgn(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
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

pub(in crate::markdown) async fn directive_mermaid(
    d: &Directive,
    ctx: &mut RenderCtx<'_>,
) -> String {
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
