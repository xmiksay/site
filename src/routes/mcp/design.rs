//! MCP tools for the design draft (`design_*`, #118): a thin adapter over
//! the shared `crate::design::tools`, which the AI assistant uses too.

use serde_json::{Value, json};

use crate::design::tools::{self, Ctx, Output, specs};
use crate::state::AppState;

use super::rpc::{JsonRpcResponse, json_result, tool_error, tool_result};

/// Whether `name` is a design tool this module dispatches.
pub(super) fn handles(name: &str) -> bool {
    specs::find(name).is_some()
}

/// The `tools/list` entries of the design tools.
pub(super) fn tool_list() -> Vec<Value> {
    specs::TOOLS
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": spec.schema(),
            })
        })
        .collect()
}

pub(super) async fn call(
    state: &AppState,
    id: Option<Value>,
    name: &str,
    arguments: Value,
) -> JsonRpcResponse {
    let ctx = Ctx {
        db: &state.db,
        storage: &state.storage,
        design: &state.design,
        hub: &state.ws_hub,
    };
    match tools::call(&ctx, name, arguments).await {
        Ok(Output::Text(text)) => tool_result(id, text),
        Ok(Output::Json(value)) => json_result(id, value),
        Err(e) => tool_error(id, &e.0),
    }
}
