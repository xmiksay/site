//! `<fen>`, `<pgn>`, `<mermaid>` directives — file-backed or inline-body forms.

use bytes::Bytes;
use chess_diagram::Renderer as _;
use minijinja::context;

use super::super::RenderCtx;
use super::super::directives::Directive;
use super::super::lookup::{fetch_file, lookup_label, parse_file_lookup};
use super::super::renderer::{block, render_md_template};
use super::{TextBlob, inline_body, markdown_image, parse_size_class, read_text_blob};

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
            match read_text_blob(ctx.db, &file.hash).await {
                TextBlob::Found(fen) => fen,
                TextBlob::NotFound => {
                    let html = format!(r#"<p><em>[fen file "{}" not found]</em></p>"#, file.path);
                    return block(html);
                }
                TextBlob::InvalidUtf8 => {
                    let html = format!(
                        r#"<p><em>[fen "{}": stored file is not valid UTF-8 text]</em></p>"#,
                        file.path
                    );
                    return block(html);
                }
            }
        }
    };

    if ctx.export.is_some() {
        return match chess_diagram::render_svg(fen.trim(), &chess_diagram::Options::default()) {
            Ok(svg) => {
                let key = format!(
                    "bridge/fen/{}.svg",
                    crate::files::hash_blob(fen.trim().as_bytes())
                );
                if let Some(assets) = ctx.export.as_mut() {
                    assets.push((key.clone(), Bytes::from(svg)));
                }
                block(markdown_image("Chess position", &key))
            }
            Err(e) => block(format!("*[fen: invalid position: {e}]*")),
        };
    }

    let html = render_md_template(
        ctx,
        "fen",
        context! { fen => fen.trim(), size_class => size_class },
    );
    block(html)
}

/// Which ply a `<pgn move="...">` attribute is asking for, before resolving
/// it against the PGN's actual length (which requires calling
/// `chess_diagram::pgn::board_at` — this function is pure attribute parsing).
#[derive(Debug, PartialEq, Eq)]
pub(in crate::markdown) enum PgnPlyRequest {
    /// `move` absent, `"last"`, or unparseable — resolve to the final
    /// position once the PGN's real ply count is known.
    Last,
    /// An explicit ply, to pass to `board_at` verbatim (including an
    /// out-of-range value, which must surface as a visible error rather than
    /// silently clamp — only `Last`'s own resolution clamps).
    Ply(usize),
}

/// Parse a `<pgn move="...">` attribute into a [`PgnPlyRequest`]. `move="N"`
/// already matches `chess_diagram::pgn::board_at`'s own ply semantics (0 =
/// start, 1 = after White's first half-move) with no off-by-one, confirmed
/// against `design/assets/js/chess-viewer.js`'s `data-move` handling.
/// A value that doesn't parse as an integer (an author typo) falls back to
/// `Last` — the final position is the least-surprising default, matching
/// what an absent/`"last"` attribute already does.
pub(in crate::markdown) fn pgn_ply_request(move_attr: Option<&str>) -> PgnPlyRequest {
    match move_attr {
        None | Some("last") => PgnPlyRequest::Last,
        Some("first") | Some("0") => PgnPlyRequest::Ply(0),
        Some(other) => match other.parse::<usize>() {
            Ok(n) => PgnPlyRequest::Ply(n),
            Err(_) => PgnPlyRequest::Last,
        },
    }
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
            match read_text_blob(ctx.db, &file.hash).await {
                TextBlob::Found(pgn) => pgn,
                TextBlob::NotFound => {
                    let html = format!(r#"<p><em>[pgn file "{}" not found]</em></p>"#, file.path);
                    return block(html);
                }
                TextBlob::InvalidUtf8 => {
                    let html = format!(
                        r#"<p><em>[pgn "{}": stored file is not valid UTF-8 text]</em></p>"#,
                        file.path
                    );
                    return block(html);
                }
            }
        }
    };
    let pgn = pgn.trim().to_string();

    if ctx.export.is_some() {
        let request = pgn_ply_request(move_attr);
        let is_last = matches!(request, PgnPlyRequest::Last);
        let initial_ply = match request {
            PgnPlyRequest::Ply(n) => n,
            PgnPlyRequest::Last => usize::MAX,
        };
        // `usize::MAX` is our own internal "last" sentinel — resolve it against
        // the PGN's real ply count on the resulting `PlyOutOfRange`. An
        // explicit out-of-range `move="N"` from a page author (not the
        // sentinel) must still surface as a visible error, never silently
        // clamp.
        let resolved = match chess_diagram::pgn::board_at(&pgn, initial_ply) {
            Ok(board) => Ok((initial_ply, board)),
            Err(chess_diagram::pgn::PgnError::PlyOutOfRange { available, .. }) if is_last => {
                chess_diagram::pgn::board_at(&pgn, available).map(|board| (available, board))
            }
            Err(e) => Err(e),
        };
        return match resolved {
            Ok((resolved_ply, board)) => {
                let svg =
                    chess_diagram::SvgRenderer.render(&board, &chess_diagram::Options::default());
                let key = format!(
                    "bridge/pgn/{}-{resolved_ply}.svg",
                    crate::files::hash_blob(pgn.as_bytes())
                );
                if let Some(assets) = ctx.export.as_mut() {
                    assets.push((key.clone(), Bytes::from(svg)));
                }
                block(markdown_image("Chess position", &key))
            }
            Err(e) => block(format!("*[pgn: {e}]*")),
        };
    }

    let html = render_md_template(
        ctx,
        "pgn",
        context! {
            pgn => pgn.as_str(),
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
            match read_text_blob(ctx.db, &file.hash).await {
                TextBlob::Found(src) => src,
                TextBlob::NotFound => {
                    let html = format!(
                        r#"<p><em>[mermaid file "{}" not found]</em></p>"#,
                        file.path
                    );
                    return block(html);
                }
                TextBlob::InvalidUtf8 => {
                    let html = format!(
                        r#"<p><em>[mermaid "{}": stored file is not valid UTF-8 text]</em></p>"#,
                        file.path
                    );
                    return block(html);
                }
            }
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

    if ctx.export.is_some() {
        return if !svg.is_empty() {
            let key = format!(
                "bridge/mermaid/{}.svg",
                crate::files::hash_blob(source.as_bytes())
            );
            if let Some(assets) = ctx.export.as_mut() {
                assets.push((key.clone(), Bytes::from(svg)));
            }
            block(markdown_image("Diagram", &key))
        } else {
            // Not `block()`: that helper strips blank-only lines to protect a
            // CommonMark raw-HTML block, which doesn't apply to a plain
            // markdown fence and would corrupt it.
            format!("\n\n```text\n{source}\n```\n\n")
        };
    }

    let html = render_md_template(
        ctx,
        "mermaid",
        context! { svg => svg, source => source, size_class => size_class },
    );
    block(html)
}
