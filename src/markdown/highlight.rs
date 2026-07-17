//! Server-side syntax highlighting of fenced code blocks.

use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, Tag, TagEnd};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

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
pub(super) fn highlight_code_block_events(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
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
pub(super) fn highlight_code_block(lang: &str, code: &str) -> String {
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
