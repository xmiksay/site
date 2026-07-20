use std::collections::HashMap;

use super::directives::{Directive, parse_tag_directive};
use super::handlers::{
    PgnPlyRequest, TextBlob, decode_text_blob, json_table, markdown_image, markdown_table,
    parse_size_class, pgn_ply_request, run_jq,
};
use super::highlight::highlight_code_block;
use super::lookup::{FileLookup, parse_file_lookup};
use super::renderer::{block, collect_container, collect_mermaid_fence};

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

#[test]
fn decode_text_blob_found() {
    let result = decode_text_blob(Some(b"1. e4 e5".to_vec()));
    assert_eq!(result, TextBlob::Found("1. e4 e5".to_string()));
}

#[test]
fn decode_text_blob_not_found() {
    let result = decode_text_blob(None);
    assert_eq!(result, TextBlob::NotFound);
}

#[test]
fn decode_text_blob_invalid_utf8() {
    // Lone continuation byte: not valid UTF-8 on its own.
    let result = decode_text_blob(Some(vec![0x80, 0x81]));
    assert_eq!(result, TextBlob::InvalidUtf8);
}

#[test]
fn block_strips_whitespace_only_lines() {
    let html = block("<div>\n  \n<span>x</span>\n\t\n</div>".to_string());
    let inner_lines: Vec<&str> = html.trim().lines().collect();
    assert!(
        inner_lines.iter().all(|line| !line.trim().is_empty()),
        "blank line survived block(): {html:?}"
    );
    assert!(html.contains("<span>x</span>"));
}

#[test]
fn block_wraps_with_leading_trailing_blank_lines() {
    let html = block("<p>ok</p>".to_string());
    assert_eq!(html, "\n\n<p>ok</p>\n\n");
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

// ---------------------------------------------------------------------------
// Export bridge (#66): chess-diagram shape checks, `move`-attr ply
// resolution, `markdown_table`/`markdown_image` helpers.
// ---------------------------------------------------------------------------

#[test]
fn chess_diagram_render_svg_start_position() {
    let svg = chess_diagram::render_svg(
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        &chess_diagram::Options::default(),
    )
    .expect("valid FEN must render");
    assert!(svg.starts_with("<svg"));
}

#[test]
fn chess_diagram_render_svg_invalid_fen_errors() {
    assert!(chess_diagram::render_svg("not a fen", &chess_diagram::Options::default()).is_err());
}

#[test]
fn pgn_ply_request_none_or_last_means_last() {
    assert_eq!(pgn_ply_request(None), PgnPlyRequest::Last);
    assert_eq!(pgn_ply_request(Some("last")), PgnPlyRequest::Last);
}

#[test]
fn pgn_ply_request_first_or_zero_means_ply_zero() {
    assert_eq!(pgn_ply_request(Some("first")), PgnPlyRequest::Ply(0));
    assert_eq!(pgn_ply_request(Some("0")), PgnPlyRequest::Ply(0));
}

#[test]
fn pgn_ply_request_numeric() {
    assert_eq!(pgn_ply_request(Some("5")), PgnPlyRequest::Ply(5));
}

#[test]
fn pgn_ply_request_unparseable_falls_back_to_last() {
    // Documented graceful fallback for an author typo: treat it the same as
    // an absent/`"last"` attribute rather than erroring or panicking.
    assert_eq!(pgn_ply_request(Some("not a number")), PgnPlyRequest::Last);
}

#[test]
fn markdown_table_basic_header_and_rows() {
    let columns = vec!["a".to_string(), "b".to_string()];
    let rows = vec![
        vec!["1".to_string(), "x".to_string()],
        vec!["2".to_string(), "y".to_string()],
    ];
    let table = markdown_table(&columns, &rows);
    assert_eq!(table, "| a | b |\n|---|---|\n| 1 | x |\n| 2 | y |\n");
}

#[test]
fn markdown_table_empty_columns_sized_to_first_row() {
    let columns: Vec<String> = Vec::new();
    let rows = vec![vec!["1".to_string(), "2".to_string(), "3".to_string()]];
    let table = markdown_table(&columns, &rows);
    assert_eq!(table, "|  |  |  |\n|---|---|---|\n| 1 | 2 | 3 |\n");
}

#[test]
fn markdown_table_escapes_pipes_in_cells() {
    let columns = vec!["a".to_string()];
    let rows = vec![vec!["1 | 2".to_string()]];
    let table = markdown_table(&columns, &rows);
    assert!(table.contains("1 \\| 2"));
}

#[test]
fn markdown_table_empty_input_is_empty_string() {
    let columns: Vec<String> = Vec::new();
    let rows: Vec<Vec<String>> = Vec::new();
    assert_eq!(markdown_table(&columns, &rows), "");
}

#[test]
fn markdown_image_basic_format() {
    assert_eq!(
        markdown_image("Chess position", "bridge/fen/abc.svg"),
        "![Chess position](bridge/fen/abc.svg)"
    );
}
