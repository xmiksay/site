//! JSON-RPC 2.0 envelope types for the MCP endpoint (`POST /mcp`, see
//! `super::handle`), plus the tool-response helpers every tool family
//! (`pages`, `tags`, `files`, `galleries`) builds its `JsonRpcResponse` from.
//! Kept separate from the per-family tool implementations so those stay
//! focused on business logic instead of JSON-RPC plumbing.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::mcp_args;

#[derive(Deserialize)]
pub(super) struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    pub(super) id: Option<Value>,
    pub(super) method: String,
    #[serde(default)]
    pub(super) params: Option<Value>,
}

#[derive(Serialize)]
pub(super) struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl JsonRpcResponse {
    pub(super) fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(super) fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

pub(super) fn tool_result(id: Option<Value>, text: String) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "content": [{
                "type": "text",
                "text": text
            }]
        }),
    )
}

pub(super) fn tool_error(id: Option<Value>, message: &str) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        json!({
            "isError": true,
            "content": [{
                "type": "text",
                "text": message
            }]
        }),
    )
}

pub(super) fn json_result(id: Option<Value>, value: Value) -> JsonRpcResponse {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    tool_result(id, text)
}

pub(super) fn parse_args<T: serde::de::DeserializeOwned>(
    id: Option<Value>,
    arguments: Value,
) -> Result<T, JsonRpcResponse> {
    mcp_args::parse_value(arguments).map_err(|e| tool_error(id, &e))
}
