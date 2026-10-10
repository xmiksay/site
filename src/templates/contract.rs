//! The design template contract: every template the site renders, its typed
//! context (from [`super::context`]) and the conventions around it, as
//! Markdown ([`markdown`]) and JSON Schema ([`json_schema`]). Committed as
//! `docs/design-contract.md` and `docs/design-contract.schema.json`
//! (`make contract`); a test fails on drift.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use serde_json::{Map, Value};

use super::context::*;
use super::samples;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplateKind {
    /// Extended by every page template.
    Layout,
    /// Rendered for a request.
    Page,
    /// Rendered for one markdown directive inside a page body.
    Partial,
}

/// One template the site renders.
pub struct TemplateSpec {
    /// Name under `templates/`.
    pub name: &'static str,
    pub kind: TemplateKind,
    /// When it renders.
    pub about: &'static str,
    schema: fn(&mut SchemaGenerator) -> Schema,
    samples: fn() -> Vec<Value>,
}

impl TemplateSpec {
    /// Example contexts; the first one is the contract's example.
    pub fn samples(&self) -> Vec<Value> {
        (self.samples)()
    }
}

fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.root_schema_for::<T>()
}

/// Infallible for these plain structs; a failure would show up in the
/// contract snapshot rather than abort.
fn to_json(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or_else(|e| Value::String(e.to_string()))
}

/// A page-template example as an admin sees it, then as a visitor.
fn logged_in_and_out<T: Serialize>(sample: fn(bool) -> T) -> Vec<Value> {
    vec![to_json(sample(true)), to_json(sample(false))]
}

pub const TEMPLATES: &[TemplateSpec] = &[
    TemplateSpec {
        name: "base.html",
        kind: TemplateKind::Layout,
        about: "Page skeleton every page template extends (`{% extends \"base.html\" %}`); \
                children fill `{% block title %}` and `{% block content %}`.",
        schema: schema_of::<Layout>,
        samples: || logged_in_and_out(samples::layout),
    },
    TemplateSpec {
        name: "path_page.html",
        kind: TemplateKind::Page,
        about: "Any other path: the menu item, else the page stored at that path.",
        schema: schema_of::<PathPageContext>,
        samples: || {
            let mut out = logged_in_and_out(samples::path_page_page);
            out.extend(logged_in_and_out(samples::path_page_menu));
            out
        },
    },
    TemplateSpec {
        name: "page_search.html",
        kind: TemplateKind::Page,
        about: "`/search?q=&tag=&path=&limit=&offset=`.",
        schema: schema_of::<PageSearchContext>,
        samples: || {
            let mut out = logged_in_and_out(samples::page_search);
            out.extend(logged_in_and_out(samples::page_search_query));
            out
        },
    },
    TemplateSpec {
        name: "404.html",
        kind: TemplateKind::Page,
        about: "A path with no menu item or page, or a private one for an anonymous visitor.",
        schema: schema_of::<Layout>,
        samples: || logged_in_and_out(samples::layout),
    },
    TemplateSpec {
        name: "markdown/page.html",
        kind: TemplateKind::Partial,
        about: "`<page path=\"…\">` / `<page id=\"N\">`.",
        schema: schema_of::<PagePartial>,
        samples: || vec![to_json(samples::page_partial())],
    },
    TemplateSpec {
        name: "markdown/img.html",
        kind: TemplateKind::Partial,
        about: "`<image path=\"…\" alt=\"…\">`, and `<file>` of an `image/*` file.",
        schema: schema_of::<ImgPartial>,
        samples: || vec![to_json(samples::img_partial())],
    },
    TemplateSpec {
        name: "markdown/file.html",
        kind: TemplateKind::Partial,
        about: "`<file path=\"…\">` of any other file.",
        schema: schema_of::<FilePartial>,
        samples: || vec![to_json(samples::file_partial())],
    },
    TemplateSpec {
        name: "markdown/gallery.html",
        kind: TemplateKind::Partial,
        about: "`<gallery path=\"…\">` / `<gallery id=\"N\">`.",
        schema: schema_of::<GalleryPartial>,
        samples: || vec![to_json(samples::gallery_partial())],
    },
    TemplateSpec {
        name: "markdown/fen.html",
        kind: TemplateKind::Partial,
        about: "`<fen>`; the board is drawn client-side by `assets/js/chess-viewer.js`.",
        schema: schema_of::<FenPartial>,
        samples: || vec![to_json(samples::fen_partial())],
    },
    TemplateSpec {
        name: "markdown/pgn.html",
        kind: TemplateKind::Partial,
        about: "`<pgn>`; the viewer is driven client-side by `assets/js/chess-viewer.js`.",
        schema: schema_of::<PgnPartial>,
        samples: || vec![to_json(samples::pgn_partial())],
    },
    TemplateSpec {
        name: "markdown/mermaid.html",
        kind: TemplateKind::Partial,
        about: "`<mermaid>` and ```` ```mermaid ```` fences.",
        schema: schema_of::<MermaidPartial>,
        samples: || vec![to_json(samples::mermaid_partial())],
    },
    TemplateSpec {
        name: "markdown/json.html",
        kind: TemplateKind::Partial,
        about: "`<json query=\"…\" type=\"table\">`.",
        schema: schema_of::<JsonPartial>,
        samples: || vec![to_json(samples::json_partial())],
    },
];

/// Every template's context schema plus the shared `$defs`.
fn schemas() -> (Vec<(&'static TemplateSpec, Value)>, Map<String, Value>) {
    let mut generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let mut out = Vec::new();
    for spec in TEMPLATES {
        let mut schema = (spec.schema)(&mut generator).to_value();
        if let Some(obj) = schema.as_object_mut() {
            obj.remove("$defs");
            obj.remove("$schema");
        }
        out.push((spec, schema));
    }
    let defs = generator.definitions().clone();
    (out, defs)
}

/// The contract as one JSON Schema document: `templates.<name>` is each
/// template's context, `$defs` the shared types.
pub fn json_schema() -> Value {
    let (schemas, defs) = schemas();
    let templates: Map<String, Value> = schemas
        .into_iter()
        .map(|(spec, schema)| (spec.name.to_string(), schema))
        .collect();
    canonical(&serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Site design template contract",
        "templates": templates,
        "$defs": defs,
    }))
}

/// Object keys sorted at every level, so the output does not depend on
/// whether serde_json's `preserve_order` happens to be enabled.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            sorted(map)
                .into_iter()
                .map(|(k, v)| (k.clone(), canonical(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn sorted(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    entries
}

/// Human-readable type of a property schema; `$ref`s link to the type's section.
fn type_label(schema: &Value) -> String {
    if let Some(name) = schema.get("$ref").and_then(Value::as_str) {
        let name = name.rsplit('/').next().unwrap_or(name);
        return format!("[`{name}`](#{})", name.to_lowercase());
    }
    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        return variants
            .iter()
            .map(type_label)
            .collect::<Vec<_>>()
            .join(" or ");
    }
    let one = |t: &str| match t {
        "array" => format!(
            "list of {}",
            schema.get("items").map(type_label).unwrap_or_default()
        ),
        "null" => "none".to_string(),
        other => other.to_string(),
    };
    match schema.get("type") {
        Some(Value::String(t)) => one(t),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .map(one)
            .collect::<Vec<_>>()
            .join(" or "),
        _ => "any".to_string(),
    }
}

fn properties_table(schema: &Value, column: &str, out: &mut String) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    out.push_str(&format!(
        "| {column} | Type | Description |\n|---|---|---|\n"
    ));
    for (name, prop) in sorted(props) {
        let desc = prop
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace('\n', " ")
            .replace('|', "\\|");
        out.push_str(&format!(
            "| `{name}` | {} | {desc} |\n",
            type_label(prop).replace('|', "\\|")
        ));
    }
    out.push('\n');
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(&canonical(value)).unwrap_or_else(|e| e.to_string())
}

/// The contract as Markdown — what `docs/design-contract.md` holds.
pub fn markdown() -> String {
    render_markdown(true)
}

/// [`markdown`] without the example contexts — the `designer` AI profile's
/// prompt carries this.
pub fn compact_markdown() -> String {
    render_markdown(false)
}

fn render_markdown(examples: bool) -> String {
    let (schemas, defs) = schemas();
    let mut out = String::from(include_str!("contract_intro.md"));
    for (spec, schema) in &schemas {
        let kind = match spec.kind {
            TemplateKind::Layout => "layout",
            TemplateKind::Page => "page",
            TemplateKind::Partial => "directive partial",
        };
        out.push_str(&format!(
            "### `{}` ({kind})\n\n{}\n\n",
            spec.name, spec.about
        ));
        properties_table(schema, "Variable", &mut out);
        if let Some(example) = spec.samples().first().filter(|_| examples) {
            out.push_str(&format!(
                "<details><summary>Example context</summary>\n\n```json\n{}\n```\n\n</details>\n\n",
                pretty(example)
            ));
        }
    }
    out.push_str("## Types\n\n");
    for (name, def) in sorted(&defs) {
        out.push_str(&format!("### `{name}`\n\n"));
        if let Some(desc) = def.get("description").and_then(Value::as_str) {
            out.push_str(&format!("{desc}\n\n"));
        }
        properties_table(def, "Field", &mut out);
    }
    out
}

/// [`json_schema`] pretty-printed — what `docs/design-contract.schema.json` holds.
pub fn json_schema_text() -> String {
    format!("{}\n", pretty(&json_schema()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_contract_is_current() {
        let committed = include_str!("../../docs/design-contract.md");
        assert!(
            committed == markdown(),
            "docs/design-contract.md is stale — run `make contract` and commit it"
        );
        let committed = include_str!("../../docs/design-contract.schema.json");
        assert!(
            committed == json_schema_text(),
            "docs/design-contract.schema.json is stale — run `make contract` and commit it"
        );
    }

    #[test]
    fn compact_contract_drops_only_the_examples() {
        let (full, compact) = (markdown(), compact_markdown());
        assert!(full.contains("Example context"));
        assert!(!compact.contains("Example context"));
        assert!(compact.len() < full.len());
        for spec in TEMPLATES {
            assert!(
                compact.contains(&format!("### `{}`", spec.name)),
                "{}",
                spec.name
            );
        }
    }

    #[test]
    fn every_baked_template_is_in_the_contract() {
        let design = crate::design::DesignStore::new(None);
        let mut baked = design.template_names();
        baked.sort();
        let mut listed: Vec<String> = TEMPLATES.iter().map(|t| t.name.to_string()).collect();
        listed.sort();
        assert_eq!(baked, listed);
    }

    #[test]
    fn type_labels_cover_options_lists_and_refs() {
        let opt = serde_json::json!({"type": ["string", "null"]});
        assert_eq!(type_label(&opt), "string or none");
        let list = serde_json::json!({"type": "array", "items": {"$ref": "#/$defs/PageView"}});
        assert_eq!(type_label(&list), "list of [`PageView`](#pageview)");
        let any_of = serde_json::json!({"anyOf": [{"$ref": "#/$defs/TagView"}, {"type": "null"}]});
        assert_eq!(type_label(&any_of), "[`TagView`](#tagview) or none");
    }

    #[test]
    fn json_schema_lists_every_template() {
        let schema = json_schema();
        for spec in TEMPLATES {
            assert!(schema["templates"][spec.name].is_object(), "{}", spec.name);
        }
        assert!(schema["$defs"]["PageView"].is_object());
    }
}
