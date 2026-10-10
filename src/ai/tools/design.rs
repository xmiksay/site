//! Design-draft tools (`design_*`, #118): one generic adapter over the
//! shared `crate::design::tools`, which the MCP server uses too, so the two
//! edges advertise and run exactly the same tools.

use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use entanglement_core::SessionId;
use entanglement_provider::ContentPart;
use entanglement_runtime::{Tool, ToolRegistry};
use sea_orm::DatabaseConnection;
use serde_json::Value;

use super::common::{ok_json, ok_text, parse_args};
use crate::design::DesignStore;
use crate::design::tools::{self, Ctx, Output, specs};
use crate::routes::ws::WsHub;
use crate::storage::Storage;

/// What every design tool shares.
pub struct DesignDeps {
    pub db: Arc<DatabaseConnection>,
    pub storage: Storage,
    pub design: Arc<DesignStore>,
    pub ws_hub: Arc<WsHub>,
}

pub struct DesignTool {
    spec: &'static specs::Spec,
    deps: Arc<DesignDeps>,
}

/// Register every design tool.
pub fn register(reg: &mut ToolRegistry, deps: DesignDeps) {
    let deps = Arc::new(deps);
    for spec in specs::TOOLS {
        reg.register(DesignTool {
            spec,
            deps: deps.clone(),
        });
    }
}

#[async_trait]
impl Tool for DesignTool {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.spec.name)
    }
    fn description(&self) -> &str {
        self.spec.description
    }
    fn schema(&self) -> Value {
        self.spec.schema()
    }
    async fn run(&self, _input: &str) -> anyhow::Result<String> {
        anyhow::bail!("{} is session-scoped; use run_for_session", self.spec.name)
    }
    async fn run_for_session(
        &self,
        _session: &SessionId,
        _request_id: &str,
        input: &str,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let args = parse_args(input)?;
        let deps = &self.deps;
        let ctx = Ctx {
            db: &deps.db,
            storage: &deps.storage,
            design: &deps.design,
            hub: &deps.ws_hub,
        };
        match tools::call(&ctx, self.spec.name, args).await {
            Ok(Output::Text(text)) => Ok(ok_text(text)),
            Ok(Output::Json(value)) => Ok(ok_json(value)),
            Err(e) => anyhow::bail!("{}", e.0),
        }
    }
}
