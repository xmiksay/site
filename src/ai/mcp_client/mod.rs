use std::collections::HashMap;
use std::str::FromStr;

use reqwest::header::{HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai::llm::ToolSpecForProvider;

pub mod manager;
pub use manager::UserMcpManager;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub name: String,
    pub url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub forward_user_token: bool,
    /// Per-server custom HTTP headers (e.g. `Authorization: Bearer …` for an
    /// external service that uses its own token). Applied during discovery
    /// and dispatch, on top of any forwarded user token.
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
}

fn parse_custom_headers(input: &HashMap<String, String>) -> HashMap<HeaderName, HeaderValue> {
    let mut out = HashMap::with_capacity(input.len());
    for (k, v) in input {
        let Ok(name) = HeaderName::from_str(k) else {
            tracing::warn!(header = %k, "Skipping invalid MCP custom header name");
            continue;
        };
        let Ok(value) = HeaderValue::from_str(v) else {
            tracing::warn!(header = %k, "Skipping invalid MCP custom header value");
            continue;
        };
        out.insert(name, value);
    }
    out
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct ToolRouting {
    pub server_name: String,
    pub server_url: String,
    pub original_tool: String,
    pub schema: Value,
    pub description: String,
    pub forward_user_token: bool,
    pub custom_headers: HashMap<HeaderName, HeaderValue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub prefixed_name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub forward_user_token: bool,
    pub connected: bool,
    pub tools: Vec<ToolInfo>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolDispatchError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("MCP transport error: {0}")]
    Transport(String),
    #[error("Tool execution error: {0}")]
    Execution(String),
}

pub struct McpClientPool {
    tools_by_prefix: HashMap<String, ToolRouting>,
    servers: Vec<ServerInfo>,
}

impl McpClientPool {
    pub fn empty() -> Self {
        McpClientPool {
            tools_by_prefix: HashMap::new(),
            servers: Vec::new(),
        }
    }

    /// Build the pool by connecting to each enabled MCP server, listing tools,
    /// and building the prefixed routing table.
    pub async fn build(configs: &[McpServerConfig], user_token: &str) -> anyhow::Result<Self> {
        use rmcp::service::{RoleClient, RunningService, ServiceExt};
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        };

        let mut tools_by_prefix = HashMap::new();
        let mut servers: Vec<ServerInfo> = Vec::with_capacity(configs.len());

        for cfg in configs {
            let mut info = ServerInfo {
                name: cfg.name.clone(),
                url: cfg.url.clone(),
                enabled: cfg.enabled,
                forward_user_token: cfg.forward_user_token,
                connected: false,
                tools: Vec::new(),
            };

            if !cfg.enabled {
                tracing::info!(name = %cfg.name, "MCP server disabled, skipping");
                servers.push(info);
                continue;
            }

            tracing::info!(name = %cfg.name, url = %cfg.url, "Connecting to MCP server for tool discovery");

            let custom_headers = parse_custom_headers(&cfg.custom_headers);

            let mut tr_cfg = StreamableHttpClientTransportConfig::with_uri(cfg.url.as_str())
                .custom_headers(custom_headers.clone());
            if cfg.forward_user_token && !user_token.is_empty() {
                tr_cfg = tr_cfg.auth_header(user_token);
            }
            let transport = StreamableHttpClientTransport::from_config(tr_cfg);
            let client_result: Result<RunningService<RoleClient, ()>, _> =
                ().serve(transport).await;

            match client_result {
                Ok(client) => {
                    match client.peer().list_all_tools().await {
                        Ok(tools) => {
                            tracing::info!(
                                name = %cfg.name,
                                tool_count = tools.len(),
                                "Discovered tools from MCP server"
                            );
                            info.connected = true;
                            for tool in tools {
                                let prefixed = format!("{}__{}", cfg.name, tool.name);
                                let schema: Value = serde_json::to_value(&*tool.input_schema)
                                    .unwrap_or(Value::Object(serde_json::Map::new()));
                                let description =
                                    tool.description.as_deref().unwrap_or("").to_string();
                                info.tools.push(ToolInfo {
                                    name: tool.name.to_string(),
                                    prefixed_name: prefixed.clone(),
                                    description: description.clone(),
                                    schema: schema.clone(),
                                });
                                tools_by_prefix.insert(
                                    prefixed.clone(),
                                    ToolRouting {
                                        server_name: cfg.name.clone(),
                                        server_url: cfg.url.clone(),
                                        original_tool: tool.name.to_string(),
                                        schema,
                                        description,
                                        forward_user_token: cfg.forward_user_token,
                                        custom_headers: custom_headers.clone(),
                                    },
                                );
                                tracing::debug!(prefixed = %prefixed, "Registered MCP tool");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                name = %cfg.name,
                                error = %e,
                                "Failed to list tools from MCP server"
                            );
                        }
                    }
                    let ct = client.cancellation_token();
                    ct.cancel();
                }
                Err(e) => {
                    tracing::warn!(
                        name = %cfg.name,
                        url = %cfg.url,
                        error = %e,
                        "Failed to connect to MCP server for discovery"
                    );
                }
            }

            servers.push(info);
        }

        Ok(McpClientPool {
            tools_by_prefix,
            servers,
        })
    }

    pub fn servers(&self) -> &[ServerInfo] {
        &self.servers
    }

    pub fn aggregated_tool_specs(&self) -> Vec<ToolSpecForProvider> {
        self.tools_by_prefix
            .iter()
            .map(|(prefixed, routing)| ToolSpecForProvider {
                name: prefixed.clone(),
                description: routing.description.clone(),
                schema: routing.schema.clone(),
            })
            .collect()
    }

    pub async fn dispatch(
        &self,
        prefixed_name: &str,
        args: Value,
        user_token: &str,
    ) -> Result<Value, ToolDispatchError> {
        use rmcp::model::CallToolRequestParams;
        use rmcp::service::{RoleClient, RunningService, ServiceExt};
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        };

        let routing = self
            .tools_by_prefix
            .get(prefixed_name)
            .ok_or_else(|| ToolDispatchError::UnknownTool(prefixed_name.to_string()))?;

        let mut config = StreamableHttpClientTransportConfig::with_uri(routing.server_url.as_str())
            .custom_headers(routing.custom_headers.clone());
        if routing.forward_user_token && !user_token.is_empty() {
            config = config.auth_header(user_token);
        }
        let transport = StreamableHttpClientTransport::from_config(config);

        let client_result: Result<RunningService<RoleClient, ()>, _> = ().serve(transport).await;
        let client = client_result.map_err(|e| {
            ToolDispatchError::Transport(format!(
                "Failed to connect to MCP server '{}': {e}",
                routing.server_name
            ))
        })?;

        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => {
                let mut map = serde_json::Map::new();
                map.insert("value".to_string(), other);
                Some(map)
            }
        };

        let mut params = CallToolRequestParams::new(routing.original_tool.clone());
        params.arguments = arguments;

        let result = client.peer().call_tool(params).await.map_err(|e| {
            ToolDispatchError::Transport(format!(
                "Tool call '{}' on '{}' failed: {e}",
                routing.original_tool, routing.server_name
            ))
        })?;

        let ct = client.cancellation_token();
        ct.cancel();

        if result.is_error == Some(true) {
            let text = result
                .content
                .iter()
                .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ToolDispatchError::Execution(text));
        }

        let text = result
            .content
            .iter()
            .filter_map(|c| c.raw.as_text().map(|t| t.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(serde_json::json!({ "text": text }))
    }

    pub fn has_tool(&self, prefixed_name: &str) -> bool {
        self.tools_by_prefix.contains_key(prefixed_name)
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools_by_prefix.keys().cloned().collect()
    }
}
