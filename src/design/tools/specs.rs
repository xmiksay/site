//! Names, descriptions and input schemas of the design tools — the one
//! catalogue both `tools/list` on `POST /mcp` and the AI assistant's tool
//! registry advertise.

use serde_json::{Value, json};

pub struct Spec {
    pub name: &'static str,
    pub description: &'static str,
    schema: fn() -> Value,
}

impl Spec {
    pub fn schema(&self) -> Value {
        (self.schema)()
    }
}

pub const LIST: &str = "design_list";
pub const READ: &str = "design_read";
pub const WRITE: &str = "design_write";
pub const DELETE: &str = "design_delete";
pub const CHANGES: &str = "design_changes";
pub const CONTRACT: &str = "design_contract";
pub const RENDER_CHECK: &str = "design_render_check";

fn path_prop() -> Value {
    json!({
        "type": "string",
        "description": "Bundle path under templates/, assets/ or mdcast/, e.g. `templates/base.html` or `assets/css/style.css`."
    })
}

pub const TOOLS: &[Spec] = &[
    Spec {
        name: LIST,
        description: "List the design draft: every file of the draft view (the draft over the baked default bundle) with `baked` (a default exists), `overridden` (differs from the default) and `size`, plus `changes` (what publishing would change vs the live design). Optional `prefix` filters paths (e.g. `templates/` or `assets/css/`).",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "prefix": { "type": "string", "description": "Only paths starting with this, e.g. `templates/`." }
                }
            })
        },
    },
    Spec {
        name: READ,
        description: "Read one design file. UTF-8 content comes back as `data`, anything else (fonts, images) as `data_base64`, with its `mimetype`. `source`: `draft` (default; the draft's copy, else the baked default), `published` (what the live site serves) or `baked` (the shipped default).",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": path_prop(),
                    "source": { "type": "string", "enum": ["draft", "published", "baked"] }
                },
                "required": ["path"]
            })
        },
    },
    Spec {
        name: WRITE,
        description: "Write one file of the design draft (never live; not validated — run design_render_check afterwards). Provide exactly one of `data` (text: templates, CSS, JS, SVG, TOML) or `data_base64` (binary: fonts, images). Over MCP the whole request is capped at 2 MB (about 1.5 MB of decoded binary): upload larger files (fonts) with `PUT /api/design/draft/{path}` and the same Bearer token, raw body.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": path_prop(),
                    "data": { "type": "string", "description": "Text contents." },
                    "data_base64": { "type": "string", "description": "Base64-encoded binary contents." }
                },
                "required": ["path"]
            })
        },
    },
    Spec {
        name: DELETE,
        description: "Remove the draft's copy of a design file: a file with a baked default reverts to that default, any other file is gone from the draft.",
        schema: || {
            json!({
                "type": "object",
                "properties": { "path": path_prop() },
                "required": ["path"]
            })
        },
    },
    Spec {
        name: CHANGES,
        description: "What publishing the draft would change vs the live design: `{path, kind: added|modified|deleted}` per path.",
        schema: || json!({ "type": "object", "properties": {} }),
    },
    Spec {
        name: CONTRACT,
        description: "The template contract: every template, the variables its context provides (with types and example contexts), the custom filters and the strict-render rules. `schema: true` returns the same contract as JSON Schema instead of Markdown.",
        schema: || {
            json!({
                "type": "object",
                "properties": {
                    "schema": { "type": "boolean", "description": "Return the JSON Schema instead of Markdown (default false)." }
                }
            })
        },
    },
    Spec {
        name: RENDER_CHECK,
        description: "Validate the draft exactly as a publish would, without publishing: every template must compile, then a strict smoke render of every template against the site's real data and the contract's example contexts (undefined variables are errors). Returns `ok`, `compile_errors` and `render_errors` (`{template, line, message, case}`).",
        schema: || json!({ "type": "object", "properties": {} }),
    },
];

pub fn find(name: &str) -> Option<&'static Spec> {
    TOOLS.iter().find(|spec| spec.name == name)
}
