//! `<json>` directive — run a jq query (via jaq) over a JSON file blob or
//! inline body, and render the result as an HTML table.

use minijinja::context;

use super::super::RenderCtx;
use super::super::directives::Directive;
use super::super::lookup::{fetch_file, lookup_label, parse_file_lookup};
use super::super::renderer::{block, render_md_template};
use super::{TextBlob, inline_body, read_text_blob};

// ---------------------------------------------------------------------------
// <json path|id|hash=... query=".rows[]" type="table">  — file-backed, or
// <json query="..." type="...">{ ...json... }</json>     — inline body.
// Runs a jq query (jaq) over the JSON and renders the result (default: table).
// ---------------------------------------------------------------------------

pub(in crate::markdown) async fn directive_json(d: &Directive, ctx: &mut RenderCtx<'_>) -> String {
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
            match read_text_blob(ctx.db, &file.hash).await {
                TextBlob::Found(src) => src,
                TextBlob::NotFound => {
                    return format!("\n\n*[json: file \"{}\" not found]*\n\n", file.path);
                }
                TextBlob::InvalidUtf8 => {
                    return format!(
                        "\n\n*[json: \"{}\": stored file is not valid UTF-8 text]*\n\n",
                        file.path
                    );
                }
            }
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

    if ctx.export.is_some() {
        return if columns.is_empty() && rows.is_empty() {
            block("*[json: empty result]*".to_string())
        } else {
            block(markdown_table(&columns, &rows))
        };
    }

    let html = render_md_template(
        ctx,
        "json",
        context! { kind => kind, columns => columns, rows => rows },
    );
    block(html)
}

/// Render a jq result as a real markdown pipe table (rather than the HTML
/// table the live renderer builds) — mdcast's typst backend converts real
/// markdown tables, but doesn't understand raw HTML (#66).
pub(in crate::markdown) fn markdown_table(columns: &[String], rows: &[Vec<String>]) -> String {
    let ncols = if !columns.is_empty() {
        columns.len()
    } else {
        rows.first().map(Vec::len).unwrap_or(0)
    };
    if ncols == 0 {
        return String::new();
    }
    fn escape(s: &str) -> String {
        s.replace('|', "\\|").replace('\n', " ")
    }
    let header: Vec<String> = if columns.is_empty() {
        vec![String::new(); ncols]
    } else {
        columns.iter().map(|c| escape(c)).collect()
    };
    let mut out = format!("| {} |\n", header.join(" | "));
    out.push_str(&format!("|{}\n", "---|".repeat(ncols)));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| escape(c)).collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

/// Run a jq query over `input` using jaq, collecting all outputs.
pub(in crate::markdown) fn run_jq(
    query: &str,
    input: serde_json::Value,
) -> Result<Vec<serde_json::Value>, String> {
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
pub(in crate::markdown) fn json_table(
    outputs: Vec<serde_json::Value>,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
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
