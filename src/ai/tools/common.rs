//! Shared helpers for the `tools/*` port of the old `local_tools/site_tools.rs`
//! to `entanglement_runtime::tools::Tool`. `ToolCall.input` arrives as a JSON
//! *string* now (not a pre-parsed `serde_json::Value`), so every tool parses
//! its own args via [`parse_args`].

use entanglement_provider::ContentPart;
use serde_json::Value;

use crate::mcp_args;

/// One text content part for a non-empty string, none for an empty one —
/// mirrors `entanglement_runtime::tools::text_parts` (private to that crate).
pub(super) fn ok_text(text: String) -> Vec<ContentPart> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![ContentPart::text(text)]
    }
}

/// Pretty-print a JSON value and wrap it as a single text part — the
/// replacement for the old `{"text": pretty_json}` envelope, which is
/// redundant now that `ContentPart::Text` carries the text directly.
pub(super) fn ok_json(v: Value) -> Vec<ContentPart> {
    let s = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
    ok_text(s)
}

/// Parse a tool call's `input` string as a JSON object — see
/// `crate::mcp_args`, shared with the MCP server's own arg parsing.
pub(super) fn parse_args(input: &str) -> anyhow::Result<Value> {
    mcp_args::parse_str(input).map_err(|e| anyhow::anyhow!(e))
}

pub(super) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

pub(super) fn required_str(args: &Value, key: &str) -> anyhow::Result<String> {
    arg_str(args, key)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}
