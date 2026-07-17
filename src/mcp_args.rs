//! Shared JSON tool-argument parsing for the two `ToolCall`-shaped edges:
//! the hand-rolled MCP server (`src/routes/mcp.rs`, args arrive as a
//! pre-parsed `serde_json::Value`) and the AI assistant's built-in tools
//! (`src/ai/tools/*.rs`, args arrive as a JSON string). Both used to carry
//! their own parallel `parse_args` copy — this is the one place the parsing
//! rules live now.

use serde_json::Value;

/// Deserialize a `serde_json::Value` into a typed args struct.
pub fn parse_value<T: serde::de::DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|e| format!("Invalid arguments: {e}"))
}

/// Parse a tool call's `input` string as a JSON object. Empty input (no
/// arguments) is treated as `{}` rather than an error — the convention for
/// zero-arg tools (e.g. `list_tags`).
pub fn parse_str(input: &str) -> Result<Value, String> {
    if input.trim().is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(input)
        .map_err(|e| format!("invalid tool arguments (expected a JSON object): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    struct Args {
        path: String,
    }

    #[test]
    fn parse_value_deserializes_matching_shape() {
        let args: Args = parse_value(json!({ "path": "a/b" })).unwrap();
        assert_eq!(args.path, "a/b");
    }

    #[test]
    fn parse_value_reports_mismatch() {
        let err = parse_value::<Args>(json!({})).unwrap_err();
        assert!(err.starts_with("Invalid arguments:"));
    }

    #[test]
    fn parse_str_empty_input_is_empty_object() {
        assert_eq!(parse_str("").unwrap(), json!({}));
        assert_eq!(parse_str("   ").unwrap(), json!({}));
    }

    #[test]
    fn parse_str_parses_json_object() {
        assert_eq!(
            parse_str(r#"{"path":"a"}"#).unwrap(),
            json!({ "path": "a" })
        );
    }

    #[test]
    fn parse_str_rejects_invalid_json() {
        assert!(parse_str("not json").is_err());
    }
}
